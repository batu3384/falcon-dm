//! Request plumbing shared by the HTTP engine: pinned clients, manual redirect
//! following (every hop is SSRF-checked), and response header parsing.

use crate::download::DownloadError;
use crate::util::{
    resolve_public_addresses_async, sanitize_header_value, validate_fetch_url_async,
};
use reqwest::header::{
    HeaderMap, ACCEPT, ACCEPT_ENCODING, CONTENT_DISPOSITION, CONTENT_RANGE, CONTENT_TYPE, COOKIE,
    ETAG, IF_RANGE, LAST_MODIFIED, LOCATION, RANGE, REFERER, USER_AGENT,
};
use reqwest::{Client, Response};
use std::time::Duration;
use url::Url;

pub(crate) const MAX_REDIRECTS: usize = 10;
/// No bytes for this long = dead connection (not a total-time limit).
pub(crate) const STALL_TIMEOUT: Duration = Duration::from_secs(30);

/// Honest product token. A copied browser UA on a non-browser TLS stack is
/// what bot filters look for; some hosts drop such connections outright.
pub(crate) const DEFAULT_USER_AGENT: &str = concat!("FalconDM/", env!("CARGO_PKG_VERSION"));

#[derive(Clone, Default)]
pub struct HttpHeaders {
    pub cookies: Option<String>,
    pub referrer: Option<String>,
    pub user_agent: Option<String>,
    pub proxy: Option<String>,
    pub max_connections: usize,
}

fn same_origin(left: &Url, right: &Url) -> bool {
    left.scheme() == right.scheme()
        && left.host_str().map(str::to_ascii_lowercase)
            == right.host_str().map(str::to_ascii_lowercase)
        && left.port_or_known_default() == right.port_or_known_default()
}

fn youtube_referrer_ok(referrer: &str) -> bool {
    Url::parse(referrer)
        .ok()
        .and_then(|parsed| parsed.host_str().map(str::to_owned))
        .is_some_and(|host| crate::util::is_youtube_host(&host))
}

fn youtube_cdn_capture(url: &Url, headers: &HttpHeaders) -> bool {
    if url.scheme() != "https" || !crate::util::is_googlevideo_url(url.as_str()) {
        return false;
    }
    headers.cookies.as_deref().is_some_and(|c| !c.trim().is_empty())
        || headers.referrer.as_deref().is_some_and(youtube_referrer_ok)
}

/// Browser default (`strict-origin-when-cross-origin`): full URL to the same
/// origin, bare origin elsewhere, nothing on an HTTPS → HTTP downgrade.
pub(crate) fn referer_for(target: &Url, referrer: &str) -> Option<String> {
    let mut referrer = Url::parse(referrer.trim()).ok()?;
    if !matches!(referrer.scheme(), "http" | "https") {
        return None;
    }
    if same_origin(&referrer, target) {
        referrer.set_fragment(None);
        let _ = referrer.set_username("");
        let _ = referrer.set_password(None);
        return Some(sanitize_header_value(referrer.as_str()));
    }
    if referrer.scheme() == "https" && target.scheme() != "https" {
        return None;
    }
    Some(format!("{}/", referrer.origin().ascii_serialization()))
}

pub(crate) fn add_request_headers(
    request: reqwest::RequestBuilder,
    initial: &Url,
    current: &Url,
    headers: &HttpHeaders,
) -> reqwest::RequestBuilder {
    // Compressed transfer would break byte-range math.
    let mut request = request.header(ACCEPT, "*/*").header(ACCEPT_ENCODING, "identity");
    // Cookies follow the *current* URL so a redirect off-site (or a googlevideo
    // hop off-CDN) cannot leak the session to a third party.
    let send_cookies = (same_origin(initial, current) && initial.scheme() == "https")
        || youtube_cdn_capture(current, headers);
    if send_cookies {
        if let Some(value) = headers.cookies.as_deref().map(sanitize_header_value) {
            if !value.is_empty() {
                request = request.header(COOKIE, value);
            }
        }
    }
    if let Some(value) = headers.referrer.as_deref().and_then(|r| referer_for(current, r)) {
        request = request.header(REFERER, value);
    }
    let user_agent = headers
        .user_agent
        .as_deref()
        .map(sanitize_header_value)
        .filter(|value| !value.is_empty())
        .unwrap_or_else(|| DEFAULT_USER_AGENT.to_string());
    request.header(USER_AGENT, user_agent)
}

/// Client whose DNS for `url`'s host is fixed to the public addresses resolved now.
pub(crate) async fn pinned_client(url: &Url, proxy: Option<&str>) -> Result<Client, DownloadError> {
    let addresses = resolve_public_addresses_async(url).await?;
    // reqwest tries the pinned addresses in order, so one dead A record is skipped.
    let mut builder = reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .connect_timeout(crate::util::CONNECT_TIMEOUT)
        .read_timeout(STALL_TIMEOUT);
    if let Some(host) = url.host_str() {
        if host.parse::<std::net::IpAddr>().is_err() {
            builder = builder.resolve_to_addrs(host, &addresses);
        }
    }
    if let Some(proxy) = proxy.map(str::trim).filter(|p| !p.is_empty()) {
        builder = builder
            .proxy(reqwest::Proxy::all(proxy).map_err(|e| DownloadError::fatal(e.to_string()))?);
    }
    builder.build().map_err(|e| DownloadError::fatal(format!("HTTP client build failed: {e}")))
}

#[derive(Clone, Debug, Default)]
pub(crate) struct RangeRequest {
    /// First byte wanted. `None` = no Range header.
    pub start: Option<u64>,
    /// Last byte wanted (inclusive). `None` = to the end.
    pub end: Option<u64>,
    /// Entity validator for `If-Range` (resume only).
    pub if_range: Option<String>,
}

pub(crate) struct Opened {
    pub response: Response,
    pub final_url: Url,
    /// Client pinned to `final_url`'s host; reusable for more ranges of it.
    pub client: Client,
}

/// GET `from`, following redirects by hand so every hop is validated and pinned.
pub(crate) async fn open(
    initial: &Url,
    from: &Url,
    headers: &HttpHeaders,
    range: &RangeRequest,
    reuse: Option<&Client>,
) -> Result<Opened, DownloadError> {
    let mut current = from.clone();
    let mut client = match reuse {
        Some(client) => client.clone(),
        None => pinned_client(&current, headers.proxy.as_deref()).await?,
    };
    for _ in 0..=MAX_REDIRECTS {
        let mut request =
            add_request_headers(client.get(current.clone()), initial, &current, headers);
        if let Some(start) = range.start {
            let spec = match range.end {
                Some(end) => format!("bytes={start}-{end}"),
                None => format!("bytes={start}-"),
            };
            request = request.header(RANGE, spec);
            if let Some(validator) = &range.if_range {
                request = request.header(IF_RANGE, validator.as_str());
            }
        }
        let response = request
            .send()
            .await
            .map_err(|e| DownloadError::from_reqwest("HTTP request failed", &e))?;
        if !response.status().is_redirection() {
            return Ok(Opened { response, final_url: current, client });
        }
        let location = response
            .headers()
            .get(LOCATION)
            .and_then(|value| value.to_str().ok())
            .ok_or_else(|| DownloadError::fatal("HTTP redirect has no valid location"))?;
        let target = current
            .join(location)
            .map_err(|e| DownloadError::fatal(format!("Invalid redirect: {e}")))?;
        current = validate_fetch_url_async(target.as_str()).await?;
        client = pinned_client(&current, headers.proxy.as_deref()).await?;
    }
    Err(DownloadError::fatal("Too many HTTP redirects"))
}

/// `Content-Range: bytes 100-199/1000` → (100, 199, Some(1000)). `*` total → None.
pub(crate) fn parse_content_range(headers: &HeaderMap) -> Option<(u64, u64, Option<u64>)> {
    let value = headers.get(CONTENT_RANGE)?.to_str().ok()?.trim();
    let rest = value.strip_prefix("bytes")?.trim_start_matches([' ', '=']);
    let (span, total) = rest.split_once('/')?;
    let (start, end) = span.trim().split_once('-')?;
    let total = total.trim();
    Some((
        start.trim().parse().ok()?,
        end.trim().parse().ok()?,
        if total == "*" { None } else { Some(total.parse().ok()?) },
    ))
}

/// Strong ETag, else Last-Modified. Weak ETags are not valid for `If-Range`.
pub(crate) fn entity_validator(headers: &HeaderMap) -> Option<String> {
    if let Some(etag) = headers.get(ETAG).and_then(|value| value.to_str().ok()) {
        let etag = etag.trim();
        if etag.starts_with('"') && etag.len() <= 512 {
            return Some(etag.to_string());
        }
    }
    headers
        .get(LAST_MODIFIED)
        .and_then(|value| value.to_str().ok())
        .map(str::trim)
        .filter(|value| !value.is_empty() && value.len() <= 64)
        .map(str::to_string)
}

fn percent_decode_lossy(raw: &str) -> String {
    percent_encoding::percent_decode_str(raw).decode_utf8_lossy().into_owned()
}

/// `Content-Disposition` filename. RFC 5987 `filename*=UTF-8''…` wins over `filename=`.
pub(crate) fn content_disposition_filename(headers: &HeaderMap) -> Option<String> {
    let raw = headers.get(CONTENT_DISPOSITION)?;
    // Servers send raw UTF-8 here even though the grammar is ASCII.
    let value = String::from_utf8_lossy(raw.as_bytes()).into_owned();
    let mut plain = None;
    let mut extended = None;
    for part in value.split(';').skip(1) {
        let Some((key, val)) = part.split_once('=') else {
            continue;
        };
        let key = key.trim().to_ascii_lowercase();
        let val = val.trim();
        if key == "filename*" {
            // charset'lang'value
            let mut pieces = val.splitn(3, '\'');
            let charset = pieces.next().unwrap_or("").to_ascii_lowercase();
            let _lang = pieces.next();
            if let Some(encoded) = pieces.next() {
                if charset == "utf-8" || charset.is_empty() {
                    extended = Some(percent_decode_lossy(encoded));
                }
            }
        } else if key == "filename" {
            let unquoted = val.strip_prefix('"').and_then(|v| v.strip_suffix('"')).unwrap_or(val);
            let unescaped = unquoted.replace("\\\"", "\"").replace("\\\\", "\\");
            // Some servers percent-encode the plain form too.
            plain = Some(if unescaped.contains('%') {
                percent_decode_lossy(&unescaped)
            } else {
                unescaped
            });
        }
    }
    extended
        .or(plain)
        .map(|name| crate::util::sanitize_filename(&name))
        .filter(|name| !name.is_empty() && name != "falcon_download")
}

/// File extension implied by a Content-Type, for names that have none.
pub(crate) fn extension_for_content_type(headers: &HeaderMap) -> Option<&'static str> {
    let value = headers.get(CONTENT_TYPE)?.to_str().ok()?;
    let mime = value.split(';').next()?.trim().to_ascii_lowercase();
    Some(match mime.as_str() {
        "video/mp4" => "mp4",
        "video/webm" => "webm",
        "video/x-matroska" => "mkv",
        "video/quicktime" => "mov",
        "video/mp2t" => "ts",
        "audio/mpeg" => "mp3",
        "audio/mp4" | "audio/x-m4a" | "audio/m4a" => "m4a",
        "audio/flac" | "audio/x-flac" => "flac",
        "audio/ogg" => "ogg",
        "audio/wav" | "audio/x-wav" => "wav",
        "audio/webm" => "webm",
        "application/pdf" => "pdf",
        "application/zip" | "application/x-zip-compressed" => "zip",
        "application/x-7z-compressed" => "7z",
        "application/x-rar-compressed" | "application/vnd.rar" => "rar",
        "application/gzip" | "application/x-gzip" => "gz",
        "application/x-tar" => "tar",
        "application/x-apple-diskimage" => "dmg",
        "application/x-iso9660-image" => "iso",
        "application/vnd.android.package-archive" => "apk",
        "application/epub+zip" => "epub",
        "application/msword" => "doc",
        "application/vnd.openxmlformats-officedocument.wordprocessingml.document" => "docx",
        "application/vnd.openxmlformats-officedocument.spreadsheetml.sheet" => "xlsx",
        "application/vnd.openxmlformats-officedocument.presentationml.presentation" => "pptx",
        "application/json" => "json",
        "text/plain" => "txt",
        "text/csv" => "csv",
        "image/jpeg" => "jpg",
        "image/png" => "png",
        "image/gif" => "gif",
        "image/webp" => "webp",
        "image/svg+xml" => "svg",
        _ => return None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use reqwest::header::HeaderValue;

    fn headers(pairs: &[(&'static str, &str)]) -> HeaderMap {
        let mut map = HeaderMap::new();
        for (name, value) in pairs {
            map.insert(*name, HeaderValue::from_bytes(value.as_bytes()).unwrap());
        }
        map
    }

    #[test]
    fn content_range_parses_total_and_unknown_total() {
        assert_eq!(
            parse_content_range(&headers(&[("content-range", "bytes 100-199/1000")])),
            Some((100, 199, Some(1000)))
        );
        assert_eq!(
            parse_content_range(&headers(&[("content-range", "bytes 0-9/*")])),
            Some((0, 9, None))
        );
        assert_eq!(parse_content_range(&headers(&[("content-range", "items 0-9/10")])), None);
        assert_eq!(parse_content_range(&HeaderMap::new()), None);
    }

    #[test]
    fn validator_prefers_strong_etag_and_skips_weak() {
        assert_eq!(
            entity_validator(&headers(&[
                ("etag", "\"abc\""),
                ("last-modified", "Wed, 21 Oct 2015 07:28:00 GMT")
            ]))
            .as_deref(),
            Some("\"abc\"")
        );
        assert_eq!(
            entity_validator(&headers(&[
                ("etag", "W/\"abc\""),
                ("last-modified", "Wed, 21 Oct 2015 07:28:00 GMT")
            ]))
            .as_deref(),
            Some("Wed, 21 Oct 2015 07:28:00 GMT")
        );
        assert_eq!(entity_validator(&headers(&[("etag", "W/\"abc\"")])), None);
    }

    #[test]
    fn content_disposition_handles_quoted_extended_and_traversal() {
        let name =
            |value: &str| content_disposition_filename(&headers(&[("content-disposition", value)]));
        assert_eq!(
            name("attachment; filename=\"report 2026.pdf\"").as_deref(),
            Some("report 2026.pdf")
        );
        assert_eq!(name("attachment; filename=plain.zip").as_deref(), Some("plain.zip"));
        assert_eq!(
            name("attachment; filename=\"fallback.bin\"; filename*=UTF-8''%C3%B6zet%20rapor.pdf")
                .as_deref(),
            Some("özet rapor.pdf")
        );
        assert_eq!(name("attachment; filename=\"My%20File.mp4\"").as_deref(), Some("My File.mp4"));
        assert_eq!(name("attachment; filename=\"../../etc/passwd\"").as_deref(), Some("passwd"));
        assert_eq!(name("inline"), None);
        assert_eq!(name("attachment; filename=\"\""), None);
    }

    #[test]
    fn content_type_maps_to_extension() {
        assert_eq!(
            extension_for_content_type(&headers(&[("content-type", "video/mp4; codecs=avc1")])),
            Some("mp4")
        );
        assert_eq!(
            extension_for_content_type(&headers(&[("content-type", "application/octet-stream")])),
            None
        );
    }

    #[test]
    fn referer_matches_browser_default_policy() {
        let same = Url::parse("https://site.example/files/a.zip").unwrap();
        let cross = Url::parse("https://cdn.example.net/a.zip").unwrap();
        let plain = Url::parse("http://cdn.example.net/a.zip").unwrap();
        let page = "https://user:pw@site.example/page?token=1#frag";
        assert_eq!(referer_for(&same, page).as_deref(), Some("https://site.example/page?token=1"));
        assert_eq!(referer_for(&cross, page).as_deref(), Some("https://site.example/"));
        assert_eq!(referer_for(&plain, page), None);
        assert_eq!(referer_for(&same, "javascript:alert(1)"), None);
        assert_eq!(referer_for(&same, "not a url"), None);
    }

    #[test]
    fn cookies_stay_on_origin_and_requests_look_like_a_browser() {
        let initial = Url::parse("https://site.example/download/1").unwrap();
        let cdn = Url::parse("https://cdn.example.net/file.bin").unwrap();
        let headers = HttpHeaders {
            cookies: Some("sid=secret".into()),
            referrer: Some("https://site.example/page".into()),
            ..Default::default()
        };
        let client = Client::new();
        let same = add_request_headers(client.get(initial.clone()), &initial, &initial, &headers)
            .build()
            .unwrap();
        assert_eq!(same.headers().get(COOKIE).unwrap(), "sid=secret");
        assert_eq!(same.headers().get(REFERER).unwrap(), "https://site.example/page");
        assert_eq!(same.headers().get(ACCEPT_ENCODING).unwrap(), "identity");
        assert!(same.headers().get(USER_AGENT).unwrap().to_str().unwrap().starts_with("FalconDM/"));

        let cross =
            add_request_headers(client.get(cdn.clone()), &initial, &cdn, &headers).build().unwrap();
        assert!(cross.headers().get(COOKIE).is_none(), "cookies must not follow a redirect");
        assert_eq!(cross.headers().get(REFERER).unwrap(), "https://site.example/");
    }

    #[test]
    fn youtube_cdn_capture_accepts_cross_origin_session() {
        let initial =
            Url::parse("https://rr3---sn-abc.googlevideo.com/videoplayback?id=xyz").unwrap();
        let headers = HttpHeaders {
            cookies: Some("VISITOR=1".into()),
            referrer: Some("https://www.youtube.com/watch?v=abc".into()),
            ..Default::default()
        };
        assert!(youtube_cdn_capture(&initial, &headers));
        let other = Url::parse("https://cdn.example.com/file.bin").unwrap();
        assert!(!youtube_cdn_capture(&other, &headers));
    }
}
