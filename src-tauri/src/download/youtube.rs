//! Native YouTube extraction through the Innertube player API.
//!
//! The module intentionally stops at extraction: it does not route jobs, select
//! a download engine, or mux streams. Its output is a fully resolved format
//! catalog that the download orchestrator can feed to the parallel-range HTTP
//! engine. Direct VISIONOS URLs are the fast path. Ciphered formats use the
//! small, explicit compatibility solver in `youtube_cipher.rs`; unsupported
//! modern player JavaScript is an error, never a silently incomplete format.

#[path = "youtube_cipher.rs"]
mod youtube_cipher;

use crate::download::http_client::{pinned_client, MAX_REDIRECTS};
use crate::util::{is_youtube_host, sanitize_header_value, validate_fetch_url_async};
use futures::StreamExt;
use reqwest::header::{HeaderMap, HeaderValue, ACCEPT_ENCODING, CONTENT_TYPE};
use reqwest::{Method, StatusCode};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use url::Url;
use youtube_cipher::CipherError;

const DEFAULT_WEB_API_KEY: &str = "AIzaSyAO_FJ2SlqU8Q4STEHLGCilw_Y9_11qcW8";
const PLAYER_API_URL: &str = "https://www.youtube.com/youtubei/v1/player";
const YOUTUBE_ORIGIN: &str = "https://www.youtube.com";
const MAX_RESPONSE_BYTES: usize = 16 * 1024 * 1024;

/// One Innertube client definition.
///
/// Update procedure: compare this table with the current
/// `yt_dlp/extractor/youtube/_base.py::INNERTUBE_CLIENTS`, copy the exact
/// context values and numeric client id, then run a real one-off extraction of
/// a public video before changing the default order. YouTube rotates these
/// values; do not treat the table as permanent truth.
#[derive(Clone, Copy, Debug)]
pub struct InnertubeClient {
    pub key: &'static str,
    pub client_name: &'static str,
    pub client_version: &'static str,
    pub numeric_client_name: u32,
    pub device_make: Option<&'static str>,
    pub device_model: Option<&'static str>,
    pub os_name: Option<&'static str>,
    pub os_version: Option<&'static str>,
    pub user_agent: &'static str,
    pub requires_js: bool,
}

/// Ordered extraction preference. VISIONOS is first because it currently
/// returns direct HTTPS adaptive streams without a PO token or cipher for most
/// public videos. Keep this as data: adding a fallback client must not require
/// restructuring the extraction pipeline.
pub const INNERTUBE_CLIENTS: &[InnertubeClient] = &[InnertubeClient {
    key: "visionos",
    client_name: "VISIONOS",
    client_version: "1.02",
    numeric_client_name: 101,
    device_make: Some("Apple"),
    device_model: Some("RealityDevice17,1"),
    os_name: Some("visionOS"),
    os_version: Some("26.5.23O471"),
    user_agent: "Mozilla/5.0 (Macintosh; Intel Mac OS X 15_7_3) AppleWebKit/605.1.15 (KHTML, like Gecko) Version/26.0 Safari/605.1.15",
    requires_js: false,
}];

/// Validated outbound fetcher used by the extractor.
///
/// Every initial URL and redirect hop goes through `validate_fetch_url_async`
/// and is then requested with a DNS-pinned client from `http_client`. This is
/// the same SSRF boundary used by the download engines; never replace it with
/// a raw `reqwest::Client` whose DNS can be rebound after validation.
#[derive(Clone, Debug, Default)]
pub struct ValidatedFetcher {
    proxy: Option<String>,
    #[cfg(test)]
    endpoint_origin: Option<String>,
}

impl ValidatedFetcher {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn with_proxy(proxy: impl Into<String>) -> Self {
        let mut fetcher = Self::new();
        fetcher.proxy = Some(proxy.into());
        fetcher
    }

    #[cfg(test)]
    pub(crate) fn with_endpoint_origin(origin: &str) -> Self {
        Self { endpoint_origin: Some(origin.to_string()), ..Self::default() }
    }

    #[cfg(test)]
    fn wire_url(&self, raw: &str) -> String {
        let Some(origin) = &self.endpoint_origin else {
            return raw.to_string();
        };
        let Ok(parsed) = Url::parse(raw) else {
            return raw.to_string();
        };
        if !parsed.host_str().is_some_and(is_youtube_host) {
            return raw.to_string();
        }
        let mut wire = origin.trim_end_matches('/').to_string();
        wire.push_str(parsed.path());
        if let Some(query) = parsed.query() {
            wire.push('?');
            wire.push_str(query);
        }
        wire
    }

    #[cfg(not(test))]
    fn wire_url(&self, raw: &str) -> String {
        raw.to_string()
    }

    async fn get(&self, url: &str, headers: HeaderMap) -> Result<FetchResponse, ExtractError> {
        self.fetch(Method::GET, url, headers, None).await
    }

    /// Text GET for sibling extraction features (subtitles). Keeping the response
    /// type private preserves the extractor boundary while reusing the same
    /// validated/pinned network path.
    pub(crate) async fn get_text(
        &self,
        url: &str,
        headers: HeaderMap,
    ) -> Result<(u16, String), ExtractError> {
        let response = self.get(url, headers).await?;
        Ok((response.status, response.body))
    }

    async fn post_json(
        &self,
        url: &str,
        mut headers: HeaderMap,
        body: &Value,
    ) -> Result<FetchResponse, ExtractError> {
        if !headers.contains_key(CONTENT_TYPE) {
            headers.insert(CONTENT_TYPE, HeaderValue::from_static("application/json"));
        }
        let body = serde_json::to_vec(body)
            .map_err(|_| ExtractError::MalformedResponse("could not serialize player request"))?;
        self.fetch(Method::POST, url, headers, Some(body)).await
    }

    async fn fetch(
        &self,
        method: Method,
        raw_url: &str,
        headers: HeaderMap,
        body: Option<Vec<u8>>,
    ) -> Result<FetchResponse, ExtractError> {
        let logical_youtube = Url::parse(raw_url)
            .ok()
            .and_then(|url| url.host_str().map(str::to_string))
            .is_some_and(|host| is_youtube_host(&host));
        let wire_url = self.wire_url(raw_url);
        let mut current = validate_fetch_url_async(&wire_url)
            .await
            .map_err(|e| ExtractError::Network(format!("URL validation failed: {e}")))?;
        let mut client = pinned_client(&current, self.proxy.as_deref())
            .await
            .map_err(|e| ExtractError::Network(e.message()))?;
        let mut method = method;
        let mut body = body;
        let mut first_hop = true;

        for _ in 0..=MAX_REDIRECTS {
            let mut request = client.request(method.clone(), current.clone());
            request = request.header(ACCEPT_ENCODING, "identity");
            let cookie_allowed = if first_hop && logical_youtube {
                true
            } else {
                current.host_str().is_some_and(is_youtube_host)
            };
            for (name, value) in headers.iter() {
                if name.as_str().eq_ignore_ascii_case("cookie") && !cookie_allowed {
                    continue;
                }
                request = request.header(name.clone(), value.clone());
            }
            if let Some(bytes) = &body {
                request = request.body(bytes.clone());
            }
            let response = request.send().await.map_err(|e| {
                ExtractError::Network(
                    crate::download::DownloadError::from_reqwest("YouTube request failed", &e)
                        .message(),
                )
            })?;

            if response.status().is_redirection() {
                let status = response.status();
                let can_follow = match method {
                    Method::GET | Method::HEAD => true,
                    Method::POST => matches!(
                        status,
                        StatusCode::SEE_OTHER
                            | StatusCode::TEMPORARY_REDIRECT
                            | StatusCode::PERMANENT_REDIRECT
                    ),
                    _ => false,
                };
                if !can_follow {
                    return Err(ExtractError::Network(format!(
                        "unsupported YouTube redirect status {}",
                        status.as_u16()
                    )));
                }
                let location = response
                    .headers()
                    .get(reqwest::header::LOCATION)
                    .and_then(|value| value.to_str().ok())
                    .ok_or_else(|| {
                        ExtractError::Network("YouTube redirect has no valid location".into())
                    })?;
                let target = current
                    .join(location)
                    .map_err(|e| ExtractError::Network(format!("invalid YouTube redirect: {e}")))?;
                current = validate_fetch_url_async(target.as_str()).await.map_err(|e| {
                    ExtractError::Network(format!("redirect validation failed: {e}"))
                })?;
                client = pinned_client(&current, self.proxy.as_deref())
                    .await
                    .map_err(|e| ExtractError::Network(e.message()))?;
                if status == StatusCode::SEE_OTHER {
                    method = Method::GET;
                    body = None;
                }
                first_hop = false;
                continue;
            }

            let status = response.status().as_u16();
            let mut bytes = Vec::new();
            let mut stream = response.bytes_stream();
            while let Some(chunk) = stream.next().await {
                let chunk = chunk.map_err(|e| {
                    ExtractError::Network(
                        crate::download::DownloadError::from_reqwest("YouTube response failed", &e)
                            .message(),
                    )
                })?;
                if bytes.len().saturating_add(chunk.len()) > MAX_RESPONSE_BYTES {
                    return Err(ExtractError::Network("YouTube response exceeded 16 MiB".into()));
                }
                bytes.extend_from_slice(&chunk);
            }
            return Ok(FetchResponse {
                status,
                body: String::from_utf8_lossy(&bytes).into_owned(),
            });
        }
        Err(ExtractError::Network("too many YouTube redirects".into()))
    }
}

struct FetchResponse {
    status: u16,
    body: String,
}

/// Which JavaScript challenge failed.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum CipherTransform {
    Signature,
    N,
}

/// Extraction failure taxonomy. `code()` is the stable machine-readable code;
/// UI localization is deliberately outside this module.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ExtractError {
    InvalidVideoId(String),
    Network(String),
    Http { status: u16, context: &'static str },
    MalformedResponse(&'static str),
    AgeRestricted { reason: String },
    LoginRequired { reason: String },
    PrivateVideo { reason: String },
    BotCheck { reason: String },
    LiveOrPremiere { reason: String },
    Unavailable { status: String, reason: String },
    NoFormats,
    CipherUnsupported { transform: CipherTransform, reason: String },
    CipherFailed { transform: CipherTransform, reason: String },
}

impl ExtractError {
    pub fn code(&self) -> &'static str {
        match self {
            Self::InvalidVideoId(_) => "ERR_YT_INVALID_VIDEO_ID",
            Self::Network(_) => "ERR_YT_NETWORK",
            Self::Http { .. } => "ERR_YT_HTTP",
            Self::MalformedResponse(_) => "ERR_YT_MALFORMED_RESPONSE",
            Self::AgeRestricted { .. } => "ERR_YT_AGE_RESTRICTED",
            Self::LoginRequired { .. } => "ERR_YT_LOGIN_REQUIRED",
            Self::PrivateVideo { .. } => "ERR_YT_PRIVATE",
            Self::BotCheck { .. } => "ERR_YT_BOT_CHECK",
            Self::LiveOrPremiere { .. } => "ERR_YT_LIVE_OR_PREMIERE",
            Self::Unavailable { .. } => "ERR_YT_UNAVAILABLE",
            Self::NoFormats => "ERR_YT_NO_FORMATS",
            Self::CipherUnsupported { .. } => "ERR_YT_CIPHER_UNSUPPORTED",
            Self::CipherFailed { .. } => "ERR_YT_CIPHER_FAILED",
        }
    }
}

impl std::fmt::Display for ExtractError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::InvalidVideoId(input) => {
                write!(f, "{}: invalid YouTube video URL or id `{input}`", self.code())
            }
            Self::Network(message) => write!(f, "{}: {message}", self.code()),
            Self::Http { status, context } => {
                write!(f, "{}: {context} returned HTTP {status}", self.code())
            }
            Self::MalformedResponse(what) => write!(f, "{}: malformed {what}", self.code()),
            Self::AgeRestricted { reason } => write!(f, "{}: {reason}", self.code()),
            Self::LoginRequired { reason } => write!(f, "{}: {reason}", self.code()),
            Self::PrivateVideo { reason } => write!(f, "{}: {reason}", self.code()),
            Self::BotCheck { reason } => write!(f, "{}: {reason}", self.code()),
            Self::LiveOrPremiere { reason } => write!(f, "{}: {reason}", self.code()),
            Self::Unavailable { status, reason } => {
                write!(f, "{}: {status}: {reason}", self.code())
            }
            Self::NoFormats => write!(f, "{}: player response has no HTTPS formats", self.code()),
            Self::CipherUnsupported { transform, reason } => {
                write!(f, "{}: {transform:?} cipher unsupported: {reason}", self.code())
            }
            Self::CipherFailed { transform, reason } => {
                write!(f, "{}: {transform:?} cipher failed: {reason}", self.code())
            }
        }
    }
}

impl std::error::Error for ExtractError {}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Thumbnail {
    pub url: String,
    pub width: Option<u32>,
    pub height: Option<u32>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum CodecFamily {
    H264,
    Vp9,
    Av1,
    Aac,
    Opus,
    Other,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct YoutubeFormat {
    pub itag: u32,
    /// Fully resolved HTTPS URL. `signatureCipher` and the `n` challenge have
    /// already been applied when the input player response required them.
    pub url: String,
    pub mime_type: String,
    /// MIME subtype (`mp4`, `webm`, ...), lower-case.
    pub container: String,
    pub codec_family: CodecFamily,
    /// Raw `codecs=` parameter when present.
    pub codecs: Option<String>,
    pub width: Option<u32>,
    pub height: Option<u32>,
    pub bitrate: Option<u64>,
    pub content_length: Option<u64>,
    pub audio_quality: Option<String>,
    pub progressive: bool,
    pub has_video: bool,
    pub has_audio: bool,
    pub quality_label: Option<String>,
    pub fps: Option<f64>,
    pub was_ciphered: bool,
}

impl YoutubeFormat {
    pub fn is_video_only(&self) -> bool {
        self.has_video && !self.has_audio
    }

    pub fn is_audio_only(&self) -> bool {
        self.has_audio && !self.has_video
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct CaptionTrack {
    pub base_url: String,
    pub language_code: String,
    /// Raw YouTube kind. `Some("asr")` is an automatic speech recognition track.
    pub kind: Option<String>,
    pub is_automatic: bool,
    pub name: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct YoutubeExtraction {
    pub video_id: String,
    pub title: String,
    pub author: Option<String>,
    pub duration_seconds: Option<u64>,
    pub thumbnails: Vec<Thumbnail>,
    pub formats: Vec<YoutubeFormat>,
    pub captions: Vec<CaptionTrack>,
    pub hls_manifest_url: Option<String>,
    pub dash_manifest_url: Option<String>,
    pub expires_in_seconds: Option<u64>,
    pub client_name: String,
    /// Diagnostic only: the watch page's WEB client version, useful when
    /// updating `INNERTUBE_CLIENTS`. It is not sent as the VISIONOS version.
    pub web_client_version: Option<String>,
}

fn looks_like_video_id(id: &str) -> bool {
    id.len() == 11 && id.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
}

fn parse_video_id(input: &str) -> Result<String, ExtractError> {
    let trimmed = input.trim();
    if looks_like_video_id(trimmed) {
        return Ok(trimmed.to_string());
    }
    let parsed = Url::parse(trimmed)
        .map_err(|_| ExtractError::InvalidVideoId(trimmed.chars().take(120).collect()))?;
    let host = parsed.host_str().unwrap_or_default();
    if !is_youtube_host(host) {
        return Err(ExtractError::InvalidVideoId(trimmed.chars().take(120).collect()));
    }
    let candidate = if host.eq_ignore_ascii_case("youtu.be") {
        parsed.path_segments().and_then(|mut segments| segments.next()).map(str::to_string)
    } else if parsed.path() == "/watch" {
        parsed.query_pairs().find_map(|(key, value)| (key == "v").then(|| value.into_owned()))
    } else {
        parsed.path_segments().and_then(|mut segments| {
            let first = segments.next()?;
            matches!(first, "shorts" | "embed" | "live" | "v")
                .then(|| segments.next().map(str::to_string))
                .flatten()
        })
    };
    candidate
        .filter(|id| looks_like_video_id(id))
        .ok_or_else(|| ExtractError::InvalidVideoId(trimmed.chars().take(120).collect()))
}

fn insert_sanitized(headers: &mut HeaderMap, name: &'static str, value: &str) {
    let clean = sanitize_header_value(value);
    if clean.is_empty() {
        return;
    }
    if let Ok(value) = HeaderValue::from_str(&clean) {
        headers.insert(reqwest::header::HeaderName::from_static(name), value);
    }
}

fn page_headers(client: &InnertubeClient, cookies: Option<&str>) -> HeaderMap {
    let mut headers = HeaderMap::new();
    insert_sanitized(&mut headers, "user-agent", client.user_agent);
    insert_sanitized(
        &mut headers,
        "accept",
        "text/html,application/xhtml+xml,application/xml;q=0.9,*/*;q=0.8",
    );
    if let Some(cookies) = cookies {
        insert_sanitized(&mut headers, "cookie", cookies);
    }
    headers
}

fn player_headers(
    client: &InnertubeClient,
    visitor_data: Option<&str>,
    cookies: Option<&str>,
) -> HeaderMap {
    let mut headers = HeaderMap::new();
    insert_sanitized(&mut headers, "user-agent", client.user_agent);
    insert_sanitized(&mut headers, "accept", "application/json");
    insert_sanitized(
        &mut headers,
        "x-youtube-client-name",
        &client.numeric_client_name.to_string(),
    );
    insert_sanitized(&mut headers, "x-youtube-client-version", client.client_version);
    insert_sanitized(&mut headers, "origin", YOUTUBE_ORIGIN);
    if let Some(visitor) = visitor_data.filter(|value| !value.trim().is_empty()) {
        insert_sanitized(&mut headers, "x-goog-visitor-id", visitor);
    }
    if let Some(cookies) = cookies {
        insert_sanitized(&mut headers, "cookie", cookies);
    }
    headers
}

fn json_string_field(text: &str, key: &str) -> Option<String> {
    let needle = format!("\"{key}\"");
    for (index, _) in text.match_indices(&needle) {
        let bytes = text.as_bytes();
        let mut cursor = index + needle.len();
        while matches!(bytes.get(cursor), Some(byte) if byte.is_ascii_whitespace()) {
            cursor += 1;
        }
        if bytes.get(cursor) != Some(&b':') {
            continue;
        }
        cursor += 1;
        while matches!(bytes.get(cursor), Some(byte) if byte.is_ascii_whitespace()) {
            cursor += 1;
        }
        if bytes.get(cursor) != Some(&b'"') {
            continue;
        }
        let start = cursor;
        cursor += 1;
        let mut escaped = false;
        while cursor < bytes.len() {
            let byte = bytes[cursor];
            if escaped {
                escaped = false;
            } else if byte == b'\\' {
                escaped = true;
            } else if byte == b'"' {
                if let Ok(value) = serde_json::from_str::<String>(&text[start..=cursor]) {
                    return Some(value);
                }
                break;
            }
            cursor += 1;
        }
    }
    None
}

fn json_number_field(text: &str, key: &str) -> Option<u64> {
    let needle = format!("\"{key}\"");
    for (index, _) in text.match_indices(&needle) {
        let bytes = text.as_bytes();
        let mut cursor = index + needle.len();
        while matches!(bytes.get(cursor), Some(byte) if byte.is_ascii_whitespace()) {
            cursor += 1;
        }
        if bytes.get(cursor) != Some(&b':') {
            continue;
        }
        cursor += 1;
        while matches!(bytes.get(cursor), Some(byte) if byte.is_ascii_whitespace()) {
            cursor += 1;
        }
        let start = cursor;
        while matches!(bytes.get(cursor), Some(byte) if byte.is_ascii_digit()) {
            cursor += 1;
        }
        if cursor > start {
            if let Ok(value) = text[start..cursor].parse::<u64>() {
                return Some(value);
            }
        }
    }
    None
}

#[derive(Default)]
struct WatchConfig {
    api_key: Option<String>,
    web_client_version: Option<String>,
    visitor_data: Option<String>,
    player_js_url: Option<String>,
    signature_timestamp: Option<u64>,
}

fn absolutize_player_url(raw: &str) -> Option<String> {
    let absolute = if let Some(rest) = raw.strip_prefix("//") {
        format!("https://{rest}")
    } else if Url::parse(raw).is_ok() {
        raw.to_string()
    } else {
        Url::parse(YOUTUBE_ORIGIN).ok()?.join(raw).ok()?.to_string()
    };
    let parsed = Url::parse(&absolute).ok()?;
    parsed.host_str().is_some_and(is_youtube_host).then_some(absolute)
}

fn parse_watch_config(html: &str) -> WatchConfig {
    let player_js_url = json_string_field(html, "PLAYER_JS_URL")
        .or_else(|| json_string_field(html, "jsUrl"))
        .and_then(|url| absolutize_player_url(&url));
    let signature_timestamp = json_number_field(html, "STS")
        .or_else(|| json_string_field(html, "STS").and_then(|value| value.parse().ok()))
        .or_else(|| json_number_field(html, "signatureTimestamp"))
        .or_else(|| {
            json_string_field(html, "signatureTimestamp").and_then(|value| value.parse().ok())
        });
    WatchConfig {
        api_key: json_string_field(html, "INNERTUBE_API_KEY"),
        web_client_version: json_string_field(html, "INNERTUBE_CLIENT_VERSION"),
        visitor_data: json_string_field(html, "VISITOR_DATA")
            .or_else(|| json_string_field(html, "visitorData")),
        player_js_url,
        signature_timestamp,
    }
}

fn player_request_body(
    client: &InnertubeClient,
    video_id: &str,
    signature_timestamp: Option<u64>,
) -> Value {
    let mut client_context = Map::new();
    client_context.insert("clientName".into(), client.client_name.into());
    client_context.insert("clientVersion".into(), client.client_version.into());
    client_context.insert("userAgent".into(), client.user_agent.into());
    client_context.insert("hl".into(), "en".into());
    client_context.insert("timeZone".into(), "UTC".into());
    client_context.insert("utcOffsetMinutes".into(), 0.into());
    if let Some(value) = client.device_make {
        client_context.insert("deviceMake".into(), value.into());
    }
    if let Some(value) = client.device_model {
        client_context.insert("deviceModel".into(), value.into());
    }
    if let Some(value) = client.os_name {
        client_context.insert("osName".into(), value.into());
    }
    if let Some(value) = client.os_version {
        client_context.insert("osVersion".into(), value.into());
    }

    let mut playback = Map::new();
    playback.insert("html5Preference".into(), "HTML5_PREF_WANTS".into());
    if let Some(sts) = signature_timestamp {
        playback.insert("signatureTimestamp".into(), sts.into());
    }

    let mut context = Map::new();
    context.insert("client".into(), Value::Object(client_context));
    let mut content_playback = Map::new();
    content_playback.insert("contentPlaybackContext".into(), Value::Object(playback));

    Value::Object(Map::from_iter([
        ("context".into(), Value::Object(context)),
        ("videoId".into(), video_id.into()),
        ("contentCheckOk".into(), true.into()),
        ("racyCheckOk".into(), true.into()),
        ("playbackContext".into(), Value::Object(content_playback)),
    ]))
}

fn renderer_text(value: &Value) -> Option<String> {
    if let Some(simple) = value.get("simpleText").and_then(Value::as_str) {
        return (!simple.is_empty()).then(|| simple.to_string());
    }
    let runs = value.get("runs").and_then(Value::as_array)?;
    let text =
        runs.iter().filter_map(|run| run.get("text").and_then(Value::as_str)).collect::<String>();
    (!text.is_empty()).then_some(text)
}

fn playability_reason(status_object: &Value) -> String {
    if let Some(reason) = status_object.get("reason").and_then(Value::as_str) {
        if !reason.is_empty() {
            return reason.to_string();
        }
    }
    let renderer = status_object
        .get("errorScreen")
        .and_then(|screen| screen.get("playerErrorMessageRenderer"));
    renderer
        .and_then(|renderer| {
            renderer
                .get("subreason")
                .and_then(renderer_text)
                .or_else(|| renderer.get("reason").and_then(renderer_text))
        })
        .unwrap_or_default()
}

/// Returns the dedicated error for every non-OK playability status.
fn playability_error(response: &Value) -> Result<(), ExtractError> {
    let Some(status_object) = response.get("playabilityStatus").and_then(Value::as_object) else {
        return Err(ExtractError::MalformedResponse("player response without playabilityStatus"));
    };
    let status = status_object.get("status").and_then(Value::as_str).unwrap_or("ERROR");
    if status == "OK" {
        return Ok(());
    }
    let reason = playability_reason(&Value::Object(status_object.clone()));
    let reason =
        if reason.is_empty() { format!("YouTube playability status {status}") } else { reason };
    let haystack = format!("{status} {reason}").to_ascii_lowercase();

    if haystack.contains("not a bot") || haystack.contains("captcha") {
        return Err(ExtractError::BotCheck { reason });
    }
    if haystack.contains("confirm your age")
        || haystack.contains("age-restricted")
        || haystack.contains("age restricted")
        || haystack.contains("age_check")
        || haystack.contains("age_verification")
        || haystack.contains("inappropriate for some users")
    {
        return Err(ExtractError::AgeRestricted { reason });
    }
    if haystack.contains("private video") || haystack.contains("video is private") {
        return Err(ExtractError::PrivateVideo { reason });
    }
    if haystack.contains("premiere")
        || haystack.contains("live stream offline")
        || haystack.contains("upcoming")
        || haystack.contains("will begin")
        || status == "LIVE_STREAM_OFFLINE"
    {
        return Err(ExtractError::LiveOrPremiere { reason });
    }

    match status {
        "LOGIN_REQUIRED" => Err(ExtractError::LoginRequired { reason }),
        "UNPLAYABLE" | "ERROR" | "CONTENT_CHECK_REQUIRED" => {
            Err(ExtractError::Unavailable { status: status.to_string(), reason })
        }
        other => Err(ExtractError::Unavailable { status: other.to_string(), reason }),
    }
}

fn response_indicates_live(response: &Value) -> bool {
    // `isLiveContent` also stays true on finished livestream replays, so it is
    // deliberately not enough by itself. Active, upcoming and post-live-DVR
    // streams are not seekable VOD downloads and must be rejected here.
    [
        "/videoDetails/isLive",
        "/videoDetails/isUpcoming",
        "/videoDetails/isPostLiveDvr",
        "/microformat/playerMicroformatRenderer/liveBroadcastDetails/isLiveNow",
    ]
    .iter()
    .any(|pointer| response.pointer(pointer).and_then(Value::as_bool).unwrap_or(false))
        || response
            .pointer("/streamingData/hlsManifestUrl")
            .and_then(Value::as_str)
            .is_some_and(|url| url.contains("live=1"))
}

#[derive(Debug, Deserialize)]
struct RawPlayerResponse {
    #[serde(rename = "videoDetails", default)]
    video_details: Option<RawVideoDetails>,
    #[serde(rename = "streamingData", default)]
    streaming_data: Option<RawStreamingData>,
    #[serde(default)]
    captions: Option<RawCaptions>,
}

#[derive(Debug, Deserialize)]
struct RawVideoDetails {
    #[serde(default)]
    title: Option<String>,
    #[serde(default)]
    author: Option<String>,
    #[serde(rename = "lengthSeconds", default)]
    length_seconds: Option<Value>,
    #[serde(default)]
    thumbnail: Option<RawThumbnails>,
}

#[derive(Debug, Deserialize)]
struct RawThumbnails {
    #[serde(default)]
    thumbnails: Vec<RawThumbnail>,
}

#[derive(Debug, Deserialize)]
struct RawThumbnail {
    #[serde(default)]
    url: Option<String>,
    #[serde(default)]
    width: Option<u32>,
    #[serde(default)]
    height: Option<u32>,
}

#[derive(Debug, Deserialize)]
struct RawStreamingData {
    #[serde(default)]
    formats: Vec<RawFormat>,
    #[serde(rename = "adaptiveFormats", default)]
    adaptive_formats: Vec<RawFormat>,
    #[serde(rename = "hlsManifestUrl", default)]
    hls_manifest_url: Option<String>,
    #[serde(rename = "dashManifestUrl", default)]
    dash_manifest_url: Option<String>,
    #[serde(rename = "expiresInSeconds", default)]
    expires_in_seconds: Option<Value>,
}

#[derive(Debug, Deserialize)]
struct RawFormat {
    #[serde(default)]
    itag: Option<u32>,
    #[serde(default)]
    url: Option<String>,
    #[serde(rename = "signatureCipher", alias = "cipher", default)]
    signature_cipher: Option<String>,
    #[serde(rename = "mimeType", default)]
    mime_type: Option<String>,
    #[serde(default)]
    bitrate: Option<u64>,
    #[serde(default)]
    width: Option<u32>,
    #[serde(default)]
    height: Option<u32>,
    #[serde(rename = "contentLength", default)]
    content_length: Option<Value>,
    #[serde(rename = "audioQuality", default)]
    audio_quality: Option<String>,
    #[serde(rename = "qualityLabel", default)]
    quality_label: Option<String>,
    #[serde(default)]
    fps: Option<f64>,
}

#[derive(Debug, Deserialize)]
struct RawCaptions {
    #[serde(rename = "playerCaptionsTracklistRenderer", default)]
    tracklist: Option<RawCaptionTracklist>,
}

#[derive(Debug, Deserialize)]
struct RawCaptionTracklist {
    #[serde(rename = "captionTracks", default)]
    caption_tracks: Vec<RawCaptionTrack>,
}

#[derive(Debug, Deserialize)]
struct RawCaptionTrack {
    #[serde(rename = "baseUrl", default)]
    base_url: Option<String>,
    #[serde(rename = "languageCode", default)]
    language_code: Option<String>,
    #[serde(default)]
    kind: Option<String>,
    #[serde(default)]
    name: Option<Value>,
}

fn value_as_u64(value: &Value) -> Option<u64> {
    match value {
        Value::String(text) => text.trim().parse().ok(),
        Value::Number(number) => number
            .as_u64()
            .or_else(|| number.as_f64().filter(|v| v.is_finite() && *v >= 0.0).map(|v| v as u64)),
        _ => None,
    }
}

struct SignatureCipherParts {
    encrypted_signature: String,
    signature_param: String,
    url: String,
}

fn parse_signature_cipher(raw: &str) -> Result<SignatureCipherParts, ExtractError> {
    let mut encrypted_signature = None;
    let mut signature_param = None;
    let mut url = None;
    for (key, value) in url::form_urlencoded::parse(raw.as_bytes()) {
        match key.as_ref() {
            "s" => encrypted_signature = Some(value.into_owned()),
            "sp" => signature_param = Some(value.into_owned()),
            "url" => url = Some(value.into_owned()),
            _ => {}
        }
    }
    Ok(SignatureCipherParts {
        encrypted_signature: encrypted_signature
            .filter(|value| !value.is_empty())
            .ok_or(ExtractError::MalformedResponse("signatureCipher without s"))?,
        signature_param: signature_param
            .filter(|value| !value.is_empty())
            .unwrap_or_else(|| "signature".into()),
        url: url
            .filter(|value| !value.is_empty())
            .ok_or(ExtractError::MalformedResponse("signatureCipher without url"))?,
    })
}

fn query_value(raw_url: &str, wanted: &str) -> Option<String> {
    Url::parse(raw_url)
        .ok()?
        .query_pairs()
        .find_map(|(key, value)| (key == wanted).then(|| value.into_owned()))
}

/// YouTube manifest URLs carry the `n` challenge as a path segment
/// (`/n/<challenge>/`), not as the query parameter used by media URLs.
/// Return the raw segment and its byte range so replacement does not normalize
/// any other signed part of the URL.
fn manifest_n_path_challenge(raw_url: &str) -> Option<(String, std::ops::Range<usize>)> {
    let end = raw_url.find(['?', '#']).unwrap_or(raw_url.len());
    let before_query = raw_url.get(..end)?;
    let mut search_from = 0;
    while let Some(relative_marker) = before_query.get(search_from..)?.find("/n/") {
        let marker = search_from + relative_marker;
        let value_start = marker + 3;
        let relative_end = before_query.get(value_start..)?.find('/')?;
        let value_end = value_start + relative_end;
        if value_end > value_start {
            return Some((
                before_query[value_start..value_end].to_string(),
                value_start..value_end,
            ));
        }
        search_from = value_start;
    }
    None
}

fn form_encode(value: &str) -> String {
    url::form_urlencoded::byte_serialize(value.as_bytes()).collect()
}

/// Replace or append a query parameter without decoding/re-encoding unrelated
/// parameters. YouTube media URLs are signed byte-for-byte; normalizing the
/// whole query can invalidate parameters this module did not intend to touch.
fn set_query_param(raw_url: &str, key: &str, value: &str) -> Result<String, ExtractError> {
    Url::parse(raw_url).map_err(|_| ExtractError::MalformedResponse("media URL is invalid"))?;
    let (without_fragment, fragment) = match raw_url.split_once('#') {
        Some((base, fragment)) => (base, Some(fragment)),
        None => (raw_url, None),
    };
    let (base, query) = match without_fragment.split_once('?') {
        Some((base, query)) => (base, query),
        None => (without_fragment, ""),
    };
    let encoded = form_encode(value);
    let mut replaced = false;
    let mut parts = Vec::new();
    for part in query.split('&').filter(|part| !part.is_empty()) {
        let existing_key = part.split_once('=').map(|(key, _)| key).unwrap_or(part);
        if existing_key == key {
            parts.push(format!("{key}={encoded}"));
            replaced = true;
        } else {
            parts.push(part.to_string());
        }
    }
    if !replaced {
        parts.push(format!("{key}={encoded}"));
    }
    let mut out = format!("{}?{}", base, parts.join("&"));
    if let Some(fragment) = fragment {
        out.push('#');
        out.push_str(fragment);
    }
    Ok(out)
}

fn cipher_extract_error(transform: CipherTransform, error: CipherError) -> ExtractError {
    match error {
        CipherError::Unsupported(reason) => ExtractError::CipherUnsupported { transform, reason },
        CipherError::Failed(reason) => ExtractError::CipherFailed { transform, reason },
    }
}

fn codecs_from_mime(mime_type: &str) -> Option<String> {
    mime_type.split(';').skip(1).find_map(|part| {
        let part = part.trim();
        part.strip_prefix("codecs=").map(|codecs| codecs.trim_matches('"').to_string())
    })
}

fn codec_family(mime_type: &str, codecs: Option<&str>) -> CodecFamily {
    let haystack = format!(
        "{};{}",
        mime_type.to_ascii_lowercase(),
        codecs.unwrap_or_default().to_ascii_lowercase()
    );
    if haystack.contains("avc1") || haystack.contains("h264") {
        CodecFamily::H264
    } else if haystack.contains("vp09") || haystack.contains("vp9") {
        CodecFamily::Vp9
    } else if haystack.contains("av01") || haystack.contains("av1") {
        CodecFamily::Av1
    } else if haystack.contains("mp4a") || haystack.contains("aac") {
        CodecFamily::Aac
    } else if haystack.contains("opus") {
        CodecFamily::Opus
    } else {
        CodecFamily::Other
    }
}

fn container_from_mime(mime_type: &str) -> String {
    mime_type
        .split(';')
        .next()
        .and_then(|mime| {
            mime.split_once('/').map(|(_, subtype)| subtype.trim().to_ascii_lowercase())
        })
        .filter(|subtype| !subtype.is_empty())
        .unwrap_or_else(|| "unknown".into())
}

fn validate_media_url(raw: &str) -> Result<(), ExtractError> {
    let parsed =
        Url::parse(raw).map_err(|_| ExtractError::MalformedResponse("media URL is invalid"))?;
    if parsed.scheme() != "https" {
        return Err(ExtractError::MalformedResponse("media URL is not HTTPS"));
    }
    Ok(())
}

struct ResolvedStreaming {
    formats: Vec<YoutubeFormat>,
    hls_manifest_url: Option<String>,
    dash_manifest_url: Option<String>,
}

fn transform_n_in_url(
    media_url: &str,
    solver: &youtube_cipher::TransformProgram,
) -> Result<String, ExtractError> {
    let Some(challenge) = query_value(media_url, "n") else {
        return Ok(media_url.to_string());
    };
    let transformed = solver
        .apply(&challenge)
        .map_err(|error| cipher_extract_error(CipherTransform::N, error))?;
    set_query_param(media_url, "n", &transformed)
}

fn transform_n_in_manifest_url(
    manifest_url: &str,
    solver: &youtube_cipher::TransformProgram,
) -> Result<String, ExtractError> {
    if query_value(manifest_url, "n").is_some() {
        return transform_n_in_url(manifest_url, solver);
    }
    let Some((challenge, range)) = manifest_n_path_challenge(manifest_url) else {
        return Ok(manifest_url.to_string());
    };
    let transformed = solver
        .apply(&challenge)
        .map_err(|error| cipher_extract_error(CipherTransform::N, error))?;
    let mut resolved = manifest_url.to_string();
    resolved.replace_range(range, &transformed);
    Ok(resolved)
}

fn resolve_manifest_url(
    manifest_url: Option<&String>,
    n_solver: Option<&youtube_cipher::TransformProgram>,
) -> Result<Option<String>, ExtractError> {
    let Some(manifest_url) = manifest_url else {
        return Ok(None);
    };
    let has_n = query_value(manifest_url, "n").is_some()
        || manifest_n_path_challenge(manifest_url).is_some();
    let resolved = if has_n {
        let solver = n_solver.ok_or_else(|| ExtractError::CipherUnsupported {
            transform: CipherTransform::N,
            reason: "no n-transform solver was built for a manifest URL".into(),
        })?;
        transform_n_in_manifest_url(manifest_url, solver)?
    } else {
        manifest_url.clone()
    };
    validate_media_url(&resolved)?;
    Ok(Some(resolved))
}

async fn resolve_formats(
    streaming: &RawStreamingData,
    watch: &WatchConfig,
    client: &InnertubeClient,
    fetcher: &ValidatedFetcher,
    cookies: Option<&str>,
) -> Result<ResolvedStreaming, ExtractError> {
    let streams = streaming
        .formats
        .iter()
        .map(|format| (format, true))
        .chain(streaming.adaptive_formats.iter().map(|format| (format, false)))
        .collect::<Vec<_>>();
    if streams.is_empty() {
        return Err(ExtractError::NoFormats);
    }

    let mut needs_signature = false;
    let mut needs_n = false;
    for (format, _) in &streams {
        let cipher_parts = if format.url.is_none() {
            format.signature_cipher.as_deref().map(parse_signature_cipher).transpose()?
        } else {
            None
        };
        if cipher_parts.is_some() {
            needs_signature = true;
        }
        let base_url =
            format.url.as_deref().or(cipher_parts.as_ref().map(|parts| parts.url.as_str()));
        if base_url.and_then(|url| query_value(url, "n")).is_some() {
            needs_n = true;
        }
        if format.url.is_none() && cipher_parts.is_none() {
            return Err(ExtractError::MalformedResponse("format without url or signatureCipher"));
        }
    }
    for manifest_url in
        [streaming.hls_manifest_url.as_deref(), streaming.dash_manifest_url.as_deref()]
            .into_iter()
            .flatten()
    {
        if query_value(manifest_url, "n").is_some()
            || manifest_n_path_challenge(manifest_url).is_some()
        {
            needs_n = true;
        }
    }

    let mut signature_solver = None;
    let mut n_solver = None;
    if needs_signature || needs_n {
        let Some(player_url) = watch.player_js_url.as_deref() else {
            let transform =
                if needs_signature { CipherTransform::Signature } else { CipherTransform::N };
            return Err(ExtractError::CipherUnsupported {
                transform,
                reason: "watch page did not expose a player JavaScript URL".into(),
            });
        };
        let response = fetcher.get(player_url, page_headers(client, cookies)).await?;
        if !(200..300).contains(&response.status) {
            return Err(ExtractError::Http {
                status: response.status,
                context: "player JavaScript",
            });
        }
        if needs_signature {
            signature_solver = Some(
                youtube_cipher::extract_signature_transform(&response.body)
                    .map_err(|error| cipher_extract_error(CipherTransform::Signature, error))?,
            );
        }
        if needs_n {
            n_solver = Some(
                youtube_cipher::extract_n_transform(&response.body)
                    .map_err(|error| cipher_extract_error(CipherTransform::N, error))?,
            );
        }
    }

    let mut out = Vec::with_capacity(streams.len());
    for (format, progressive) in streams {
        let mut was_ciphered = false;
        let mut media_url = if let Some(url) = &format.url {
            url.clone()
        } else {
            let raw_cipher = format
                .signature_cipher
                .as_deref()
                .ok_or(ExtractError::MalformedResponse("format without url or signatureCipher"))?;
            let parts = parse_signature_cipher(raw_cipher)?;
            let solver =
                signature_solver.as_ref().ok_or_else(|| ExtractError::CipherUnsupported {
                    transform: CipherTransform::Signature,
                    reason: "no signature solver was built".into(),
                })?;
            let signature = solver
                .apply(&parts.encrypted_signature)
                .map_err(|error| cipher_extract_error(CipherTransform::Signature, error))?;
            was_ciphered = true;
            set_query_param(&parts.url, &parts.signature_param, &signature)?
        };

        if query_value(&media_url, "n").is_some() {
            let solver = n_solver.as_ref().ok_or_else(|| ExtractError::CipherUnsupported {
                transform: CipherTransform::N,
                reason: "no n-transform solver was built".into(),
            })?;
            media_url = transform_n_in_url(&media_url, solver)?;
        }
        validate_media_url(&media_url)?;

        let mime_type = format
            .mime_type
            .clone()
            .filter(|value| !value.trim().is_empty())
            .ok_or(ExtractError::MalformedResponse("format without mimeType"))?;
        let codecs = codecs_from_mime(&mime_type);
        let mime_base = mime_type.split(';').next().unwrap_or_default().trim().to_ascii_lowercase();
        let has_video = mime_base.starts_with("video/");
        let has_audio = mime_base.starts_with("audio/") || progressive;
        out.push(YoutubeFormat {
            itag: format.itag.ok_or(ExtractError::MalformedResponse("format without itag"))?,
            url: media_url,
            container: container_from_mime(&mime_type),
            codec_family: codec_family(&mime_type, codecs.as_deref()),
            codecs,
            width: format.width,
            height: format.height,
            bitrate: format.bitrate,
            content_length: format.content_length.as_ref().and_then(value_as_u64),
            audio_quality: format.audio_quality.clone(),
            progressive,
            has_video,
            has_audio,
            quality_label: format.quality_label.clone(),
            fps: format.fps,
            was_ciphered,
            mime_type,
        });
    }
    let hls_manifest_url =
        resolve_manifest_url(streaming.hls_manifest_url.as_ref(), n_solver.as_ref())?;
    let dash_manifest_url =
        resolve_manifest_url(streaming.dash_manifest_url.as_ref(), n_solver.as_ref())?;
    Ok(ResolvedStreaming { formats: out, hls_manifest_url, dash_manifest_url })
}

fn parse_captions(raw: Option<&RawCaptions>) -> Vec<CaptionTrack> {
    raw.and_then(|captions| captions.tracklist.as_ref())
        .map(|tracklist| {
            tracklist
                .caption_tracks
                .iter()
                .filter_map(|track| {
                    Some(CaptionTrack {
                        base_url: track.base_url.clone()?,
                        language_code: track.language_code.clone()?,
                        is_automatic: track.kind.as_deref() == Some("asr"),
                        kind: track.kind.clone(),
                        name: track.name.as_ref().and_then(renderer_text),
                    })
                })
                .collect()
        })
        .unwrap_or_default()
}

/// Extract metadata and fully resolved formats for a YouTube watch URL,
/// Shorts/embed/live URL, youtu.be URL, or bare 11-character video id.
///
/// This performs network I/O only through `ValidatedFetcher`. The returned
/// media URLs are time-limited and usually IP-bound by YouTube; the caller must
/// re-validate them when it later downloads them, as the HTTP engine already does.
pub async fn extract(
    video_url_or_id: &str,
    fetcher: &ValidatedFetcher,
    cookies: Option<&str>,
) -> Result<YoutubeExtraction, ExtractError> {
    let video_id = parse_video_id(video_url_or_id)?;
    let client = INNERTUBE_CLIENTS
        .first()
        .ok_or(ExtractError::MalformedResponse("empty YouTube client matrix"))?;

    let watch_url = format!("{YOUTUBE_ORIGIN}/watch?v={video_id}&hl=en");
    let watch = fetcher.get(&watch_url, page_headers(client, cookies)).await?;
    if !(200..300).contains(&watch.status) {
        return Err(ExtractError::Http { status: watch.status, context: "watch page" });
    }
    let config = parse_watch_config(&watch.body);
    let api_key = config.api_key.as_deref().unwrap_or(DEFAULT_WEB_API_KEY);
    let mut endpoint = Url::parse(PLAYER_API_URL)
        .map_err(|_| ExtractError::MalformedResponse("hard-coded player endpoint is invalid"))?;
    endpoint.query_pairs_mut().append_pair("key", api_key).append_pair("prettyPrint", "false");

    let request = player_request_body(client, &video_id, config.signature_timestamp);
    let player = fetcher
        .post_json(
            endpoint.as_str(),
            player_headers(client, config.visitor_data.as_deref(), cookies),
            &request,
        )
        .await?;
    if !(200..300).contains(&player.status) {
        return Err(ExtractError::Http { status: player.status, context: "player API" });
    }
    let response: Value = serde_json::from_str(&player.body)
        .map_err(|_| ExtractError::MalformedResponse("player API JSON"))?;
    playability_error(&response)?;
    if response_indicates_live(&response) {
        return Err(ExtractError::LiveOrPremiere {
            reason: "live or premiere streams are not VOD HTTPS extractions".into(),
        });
    }

    let raw: RawPlayerResponse = serde_json::from_value(response)
        .map_err(|_| ExtractError::MalformedResponse("player API schema"))?;
    let streaming = raw.streaming_data.as_ref().ok_or(ExtractError::NoFormats)?;
    let resolved = resolve_formats(streaming, &config, client, fetcher, cookies).await?;
    if resolved.formats.is_empty() {
        return Err(ExtractError::NoFormats);
    }

    let details = raw.video_details.as_ref();
    let thumbnails = details
        .and_then(|details| details.thumbnail.as_ref())
        .map(|thumbs| {
            thumbs
                .thumbnails
                .iter()
                .filter_map(|thumb| {
                    Some(Thumbnail {
                        url: thumb.url.clone()?,
                        width: thumb.width,
                        height: thumb.height,
                    })
                })
                .collect()
        })
        .unwrap_or_default();
    Ok(YoutubeExtraction {
        video_id: video_id.clone(),
        title: details
            .and_then(|details| details.title.clone())
            .filter(|title| !title.trim().is_empty())
            .unwrap_or_else(|| format!("YouTube video {video_id}")),
        author: details
            .and_then(|details| details.author.clone())
            .filter(|author| !author.trim().is_empty()),
        duration_seconds: details
            .and_then(|details| details.length_seconds.as_ref())
            .and_then(value_as_u64),
        thumbnails,
        formats: resolved.formats,
        captions: parse_captions(raw.captions.as_ref()),
        hls_manifest_url: resolved.hls_manifest_url,
        dash_manifest_url: resolved.dash_manifest_url,
        expires_in_seconds: streaming.expires_in_seconds.as_ref().and_then(value_as_u64),
        client_name: client.client_name.into(),
        web_client_version: config.web_client_version,
    })
}

pub(crate) fn codec_rank(format: &YoutubeFormat) -> u32 {
    match format.codec_family {
        CodecFamily::H264 => 4,
        CodecFamily::Av1 => 3,
        CodecFamily::Vp9 => 2,
        CodecFamily::Aac | CodecFamily::Opus | CodecFamily::Other => 1,
    }
}

pub(crate) fn video_rank(format: &YoutubeFormat) -> (u32, u32, u64, u32) {
    (
        format.height.unwrap_or(0),
        codec_rank(format),
        format.bitrate.unwrap_or(0),
        format.fps.map(|fps| fps.round() as u32).unwrap_or(0),
    )
}

pub(crate) fn audio_rank(format: &YoutubeFormat) -> (u64, u32, u64) {
    (
        format.bitrate.unwrap_or(0),
        match format.codec_family {
            CodecFamily::Aac => 2,
            CodecFamily::Opus => 1,
            _ => 0,
        },
        format.content_length.unwrap_or(0),
    )
}

fn best_video<'a>(formats: impl Iterator<Item = &'a YoutubeFormat>) -> Option<YoutubeFormat> {
    formats.max_by_key(|format| video_rank(format)).cloned()
}

fn best_audio<'a>(formats: impl Iterator<Item = &'a YoutubeFormat>) -> Option<YoutubeFormat> {
    formats.max_by_key(|format| audio_rank(format)).cloned()
}

/// Pick the default download pair, mirroring the intent of
/// `bestvideo[height<=1080][ext=mp4]+bestaudio[ext=m4a]/bestvideo[height<=1080]+bestaudio/best[height<=1080]/bv*+ba/b`.
///
/// The first preference is video-only MP4 at or below `max_height` plus MP4/AAC
/// audio. Within a height H.264 wins over AV1/VP9; resolution still wins over
/// codec because that is what the selector means. The tuple audio entry
/// is `None` for progressive (already muxed) media or when no separate audio
/// stream exists.
pub fn pick(
    formats: &[YoutubeFormat],
    max_height: u32,
) -> Option<(YoutubeFormat, Option<YoutubeFormat>)> {
    let video_only = || formats.iter().filter(|format| format.is_video_only());
    let audio_only = || formats.iter().filter(|format| format.is_audio_only());
    let within_cap =
        |format: &&YoutubeFormat| format.height.is_some_and(|height| height <= max_height);

    let mp4_video =
        best_video(video_only().filter(|format| within_cap(format) && format.container == "mp4"));
    let m4a_audio = best_audio(audio_only().filter(|format| {
        format.container == "mp4"
            && matches!(format.codec_family, CodecFamily::Aac | CodecFamily::Other)
    }));
    if let (Some(video), Some(audio)) = (mp4_video, m4a_audio) {
        return Some((video, Some(audio)));
    }

    let capped_video = best_video(video_only().filter(within_cap));
    let any_audio = best_audio(audio_only());
    if let (Some(video), Some(audio)) = (capped_video, any_audio) {
        return Some((video, Some(audio)));
    }

    let capped_progressive = formats
        .iter()
        .filter(|format| format.progressive)
        .filter(within_cap)
        .max_by_key(|format| video_rank(format))
        .cloned();
    if let Some(progressive) = capped_progressive {
        return Some((progressive, None));
    }

    let video = best_video(video_only());
    let audio = best_audio(audio_only());
    if let (Some(video), Some(audio)) = (video, audio) {
        return Some((video, Some(audio)));
    }

    if let Some(progressive) = formats
        .iter()
        .filter(|format| format.progressive)
        .max_by_key(|format| video_rank(format))
        .cloned()
    {
        return Some((progressive, None));
    }
    if let Some(video) = best_video(video_only()) {
        return Some((video, None));
    }
    best_audio(audio_only()).map(|audio| (audio, None))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Arc, Mutex};
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    const VIDEO_ID: &str = "dQw4w9WgXcQ";

    const WATCH_PAGE: &str = r#"<!doctype html><html><script>
ytcfg.set({"INNERTUBE_API_KEY":"fixture-key","INNERTUBE_CLIENT_VERSION":"2.20260101.00.00","VISITOR_DATA":"visitor-data","PLAYER_JS_URL":"\/s\/player\/fixture01\/base.js","STS":23123});
</script></html>"#;

    const DIRECT_PLAYER_RESPONSE: &str = r#"{
  "playabilityStatus": {"status": "OK"},
  "videoDetails": {
    "videoId": "dQw4w9WgXcQ",
    "title": "Fixture Video",
    "author": "Fixture Channel",
    "lengthSeconds": "212",
    "isLiveContent": false,
    "thumbnail": {"thumbnails": [
      {"url": "https://i.ytimg.com/vi/dQw4w9WgXcQ/default.jpg", "width": 120, "height": 90},
      {"url": "https://i.ytimg.com/vi/dQw4w9WgXcQ/maxresdefault.jpg", "width": 1280, "height": 720}
    ]}
  },
  "streamingData": {
    "expiresInSeconds": "21600",
    "hlsManifestUrl": "https://manifest.googlevideo.com/api/manifest/hls_variant/id/fixture",
    "formats": [
      {
        "itag": 18,
        "url": "https://rr1---sn-fixture.googlevideo.com/videoplayback?expire=1893456000&id=fixture&itag=18&source=youtube&requiressl=yes",
        "mimeType": "video/mp4; codecs=\"avc1.42001E, mp4a.40.2\"",
        "bitrate": 410000,
        "width": 640,
        "height": 360,
        "qualityLabel": "360p",
        "contentLength": "10800000",
        "fps": 30
      }
    ],
    "adaptiveFormats": [
      {
        "itag": 137,
        "url": "https://rr1---sn-fixture.googlevideo.com/videoplayback?expire=1893456000&id=fixture&itag=137&source=youtube&requiressl=yes",
        "mimeType": "video/mp4; codecs=\"avc1.640028\"",
        "bitrate": 4300000,
        "width": 1920,
        "height": 1080,
        "qualityLabel": "1080p",
        "contentLength": "113000000",
        "fps": 30
      },
      {
        "itag": 248,
        "url": "https://rr1---sn-fixture.googlevideo.com/videoplayback?expire=1893456000&id=fixture&itag=248&source=youtube&requiressl=yes",
        "mimeType": "video/webm; codecs=\"vp9\"",
        "bitrate": 2600000,
        "width": 1920,
        "height": 1080,
        "qualityLabel": "1080p",
        "contentLength": "68000000",
        "fps": 30
      },
      {
        "itag": 399,
        "url": "https://rr1---sn-fixture.googlevideo.com/videoplayback?expire=1893456000&id=fixture&itag=399&source=youtube&requiressl=yes",
        "mimeType": "video/mp4; codecs=\"av01.0.08M.08\"",
        "bitrate": 2400000,
        "width": 1920,
        "height": 1080,
        "qualityLabel": "1080p",
        "contentLength": "63000000",
        "fps": 30
      },
      {
        "itag": 140,
        "url": "https://rr1---sn-fixture.googlevideo.com/videoplayback?expire=1893456000&id=fixture&itag=140&source=youtube&requiressl=yes",
        "mimeType": "audio/mp4; codecs=\"mp4a.40.2\"",
        "bitrate": 128000,
        "audioQuality": "AUDIO_QUALITY_MEDIUM",
        "contentLength": "3400000"
      },
      {
        "itag": 251,
        "url": "https://rr1---sn-fixture.googlevideo.com/videoplayback?expire=1893456000&id=fixture&itag=251&source=youtube&requiressl=yes",
        "mimeType": "audio/webm; codecs=\"opus\"",
        "bitrate": 132000,
        "audioQuality": "AUDIO_QUALITY_MEDIUM",
        "contentLength": "3500000"
      }
    ]
  },
  "captions": {
    "playerCaptionsTracklistRenderer": {
      "captionTracks": [
        {
          "baseUrl": "https://www.youtube.com/api/timedtext?v=dQw4w9WgXcQ&lang=tr",
          "languageCode": "tr",
          "name": {"simpleText": "Türkçe"}
        },
        {
          "baseUrl": "https://www.youtube.com/api/timedtext?v=dQw4w9WgXcQ&lang=en&kind=asr",
          "languageCode": "en",
          "kind": "asr",
          "name": {"runs": [{"text": "English"}, {"text": " (auto)"}]}
        }
      ]
    }
  }
}"#;

    const CLASSIC_PLAYER_JS: &str = r#"
var Xh={
 r:function(a){a.reverse()},
 s:function(a,b){a.splice(0,b)},
 w:function(a,b){var c=a[0];a[0]=a[b%a.length];a[b%a.length]=c}
};
function xsig(a){a=a.split("");Xh.w(a,5);Xh.s(a,1);Xh.r(a);Xh.w(a,2);return a.join("")}
var decoder=function(x){return x.sig||xsig(x)};
function xn(a){a=a.split("");Xh.r(a);Xh.w(a,4);return a.join("")}
if (u.get("n"))&&(b=xn(b)){};
"#;

    const UNSUPPORTED_PLAYER_JS: &str = r#"
function zsig(a){a=a.split("");for(var i=0;i<a.length;i++){a[i]=a[i]}return a.join("")}
var decoder=function(x){return x.sig||zsig(x)};
"#;

    #[derive(Clone)]
    struct FixtureResponse {
        status: u16,
        content_type: &'static str,
        body: String,
    }

    #[derive(Clone, Debug)]
    struct RecordedRequest {
        method: String,
        target: String,
        headers: Vec<(String, String)>,
        body: String,
    }

    impl RecordedRequest {
        fn header(&self, wanted: &str) -> Option<&str> {
            self.headers.iter().find_map(|(name, value)| (name == wanted).then_some(value.as_str()))
        }
    }

    struct FixtureServer {
        origin: String,
        requests: Arc<Mutex<Vec<RecordedRequest>>>,
        task: tokio::task::JoinHandle<()>,
    }

    impl FixtureServer {
        fn recorded(&self) -> Vec<RecordedRequest> {
            self.requests.lock().unwrap().clone()
        }
    }

    impl Drop for FixtureServer {
        fn drop(&mut self) {
            self.task.abort();
        }
    }

    fn response(
        status: u16,
        content_type: &'static str,
        body: impl Into<String>,
    ) -> FixtureResponse {
        FixtureResponse { status, content_type, body: body.into() }
    }

    async fn start_fixture(routes: Vec<(&'static str, FixtureResponse)>) -> FixtureServer {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        let routes = Arc::new(
            routes
                .into_iter()
                .map(|(path, response)| (path.to_string(), response))
                .collect::<Vec<_>>(),
        );
        let requests = Arc::new(Mutex::new(Vec::new()));
        let task_routes = Arc::clone(&routes);
        let task_requests = Arc::clone(&requests);
        let task = tokio::spawn(async move {
            loop {
                let Ok((socket, _)) = listener.accept().await else {
                    continue;
                };
                let connection_routes = Arc::clone(&task_routes);
                let connection_requests = Arc::clone(&task_requests);
                tokio::spawn(async move {
                    handle_connection(socket, connection_routes, connection_requests).await;
                });
            }
        });
        FixtureServer { origin: format!("http://127.0.0.1:{port}"), requests, task }
    }

    async fn handle_connection(
        mut socket: tokio::net::TcpStream,
        routes: Arc<Vec<(String, FixtureResponse)>>,
        requests: Arc<Mutex<Vec<RecordedRequest>>>,
    ) {
        let mut raw = Vec::new();
        let mut chunk = [0u8; 4096];
        let header_end = loop {
            if let Some(position) = raw.windows(4).position(|window| window == b"\r\n\r\n") {
                break position + 4;
            }
            if raw.len() > 64 * 1024 {
                return;
            }
            let read = socket.read(&mut chunk).await.unwrap_or(0);
            if read == 0 {
                return;
            }
            raw.extend_from_slice(&chunk[..read]);
        };
        let head = String::from_utf8_lossy(&raw[..header_end - 4]).into_owned();
        let mut lines = head.lines();
        let mut request_parts = lines.next().unwrap_or_default().split_whitespace();
        let method = request_parts.next().unwrap_or_default().to_string();
        let target = request_parts.next().unwrap_or_default().to_string();
        let mut headers = Vec::new();
        let mut content_length = 0usize;
        for line in lines {
            let Some((name, value)) = line.split_once(':') else {
                continue;
            };
            let name = name.trim().to_ascii_lowercase();
            let value = value.trim().to_string();
            if name == "content-length" {
                content_length = value.parse().unwrap_or(0);
            }
            headers.push((name, value));
        }
        while raw.len() < header_end + content_length {
            let read = socket.read(&mut chunk).await.unwrap_or(0);
            if read == 0 {
                return;
            }
            raw.extend_from_slice(&chunk[..read]);
        }
        let body =
            String::from_utf8_lossy(&raw[header_end..header_end + content_length]).into_owned();
        requests.lock().unwrap().push(RecordedRequest {
            method: method.clone(),
            target: target.clone(),
            headers,
            body,
        });

        let path = target.split('?').next().unwrap_or_default();
        let fixture = routes
            .iter()
            .find_map(|(route, response)| (route == path).then_some(response.clone()))
            .unwrap_or_else(|| response(404, "text/plain", "fixture route not found"));
        let reason = if (200..300).contains(&fixture.status) { "OK" } else { "ERROR" };
        let wire = format!(
            "HTTP/1.1 {} {}\r\ncontent-type: {}\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{}",
            fixture.status,
            reason,
            fixture.content_type,
            fixture.body.len(),
            fixture.body
        );
        let _ = socket.write_all(wire.as_bytes()).await;
        let _ = socket.shutdown().await;
    }

    fn direct_routes() -> Vec<(&'static str, FixtureResponse)> {
        vec![
            ("/watch", response(200, "text/html", WATCH_PAGE)),
            ("/youtubei/v1/player", response(200, "application/json", DIRECT_PLAYER_RESPONSE)),
        ]
    }

    fn ciphered_player_response() -> String {
        let inner = "https://rr9---sn-cipher.googlevideo.com/videoplayback?id=ciphered&itag=137&n=abcdefg&expire=1893456000";
        let cipher = format!("s=abcdefghijklmnopqrst&sp=sig&url={}", form_encode(inner));
        serde_json::json!({
            "playabilityStatus": {"status": "OK"},
            "videoDetails": {
                "videoId": VIDEO_ID,
                "title": "Cipher Fixture",
                "author": "Fixture Channel",
                "lengthSeconds": "60",
                "isLiveContent": false
            },
            "streamingData": {
                "hlsManifestUrl": "https://manifest.googlevideo.com/api/manifest/hls_variant/n/abcdefg/file/index.m3u8",
                "adaptiveFormats": [{
                    "itag": 137,
                    "signatureCipher": cipher,
                    "mimeType": "video/mp4; codecs=\"avc1.640028\"",
                    "bitrate": 4000000,
                    "width": 1920,
                    "height": 1080,
                    "contentLength": "45000000"
                }]
            }
        })
        .to_string()
    }

    async fn extract_with_routes(
        routes: Vec<(&'static str, FixtureResponse)>,
    ) -> Result<YoutubeExtraction, ExtractError> {
        let server = start_fixture(routes).await;
        let fetcher = ValidatedFetcher::with_endpoint_origin(&server.origin);
        extract(VIDEO_ID, &fetcher, Some("SID=secret")).await
    }

    fn video_format(
        itag: u32,
        height: u32,
        container: &str,
        family: CodecFamily,
        bitrate: u64,
    ) -> YoutubeFormat {
        YoutubeFormat {
            itag,
            url: format!("https://video.example/{itag}"),
            mime_type: format!("video/{container}"),
            container: container.into(),
            codec_family: family,
            codecs: None,
            width: Some(height * 16 / 9),
            height: Some(height),
            bitrate: Some(bitrate),
            content_length: None,
            audio_quality: None,
            progressive: false,
            has_video: true,
            has_audio: false,
            quality_label: Some(format!("{height}p")),
            fps: Some(30.0),
            was_ciphered: false,
        }
    }

    fn audio_format(
        itag: u32,
        container: &str,
        family: CodecFamily,
        bitrate: u64,
    ) -> YoutubeFormat {
        YoutubeFormat {
            itag,
            url: format!("https://audio.example/{itag}"),
            mime_type: format!("audio/{container}"),
            container: container.into(),
            codec_family: family,
            codecs: None,
            width: None,
            height: None,
            bitrate: Some(bitrate),
            content_length: None,
            audio_quality: Some("AUDIO_QUALITY_MEDIUM".into()),
            progressive: false,
            has_video: false,
            has_audio: true,
            quality_label: None,
            fps: None,
            was_ciphered: false,
        }
    }

    fn progressive_format(itag: u32, height: u32, bitrate: u64) -> YoutubeFormat {
        let mut format = video_format(itag, height, "mp4", CodecFamily::H264, bitrate);
        format.progressive = true;
        format.has_audio = true;
        format
    }

    #[test]
    fn video_id_accepts_common_youtube_inputs() {
        assert_eq!(parse_video_id(VIDEO_ID).unwrap(), VIDEO_ID);
        assert_eq!(
            parse_video_id("https://www.youtube.com/watch?v=dQw4w9WgXcQ&t=12").unwrap(),
            VIDEO_ID
        );
        assert_eq!(parse_video_id("https://youtu.be/dQw4w9WgXcQ?si=abc").unwrap(), VIDEO_ID);
        assert_eq!(parse_video_id("https://www.youtube.com/shorts/dQw4w9WgXcQ").unwrap(), VIDEO_ID);
        assert_eq!(
            parse_video_id("https://www.youtube-nocookie.com/embed/dQw4w9WgXcQ").unwrap(),
            VIDEO_ID
        );
        assert!(matches!(
            parse_video_id("https://example.com/watch?v=dQw4w9WgXcQ"),
            Err(ExtractError::InvalidVideoId(_))
        ));
        assert!(matches!(parse_video_id("not-a-video!"), Err(ExtractError::InvalidVideoId(_))));
    }

    #[test]
    fn signature_cipher_uses_sp_or_signature_default() {
        let inner = form_encode("https://video.example/videoplayback?id=x");
        let explicit = parse_signature_cipher(&format!("s=abc&sp=sig&url={inner}")).unwrap();
        assert_eq!(explicit.signature_param, "sig");
        let default = parse_signature_cipher(&format!("s=abc&url={inner}")).unwrap();
        assert_eq!(default.signature_param, "signature");
        assert!(parse_signature_cipher("sp=sig&url=https%3A%2F%2Fvideo.example").is_err());
    }

    #[test]
    fn error_codes_are_stable() {
        let cases = [
            (ExtractError::InvalidVideoId("x".into()), "ERR_YT_INVALID_VIDEO_ID"),
            (ExtractError::Network("x".into()), "ERR_YT_NETWORK"),
            (ExtractError::Http { status: 500, context: "x" }, "ERR_YT_HTTP"),
            (ExtractError::MalformedResponse("x"), "ERR_YT_MALFORMED_RESPONSE"),
            (ExtractError::AgeRestricted { reason: "x".into() }, "ERR_YT_AGE_RESTRICTED"),
            (ExtractError::LoginRequired { reason: "x".into() }, "ERR_YT_LOGIN_REQUIRED"),
            (ExtractError::PrivateVideo { reason: "x".into() }, "ERR_YT_PRIVATE"),
            (ExtractError::BotCheck { reason: "x".into() }, "ERR_YT_BOT_CHECK"),
            (ExtractError::LiveOrPremiere { reason: "x".into() }, "ERR_YT_LIVE_OR_PREMIERE"),
            (
                ExtractError::Unavailable { status: "UNPLAYABLE".into(), reason: "x".into() },
                "ERR_YT_UNAVAILABLE",
            ),
            (ExtractError::NoFormats, "ERR_YT_NO_FORMATS"),
            (
                ExtractError::CipherUnsupported {
                    transform: CipherTransform::Signature,
                    reason: "x".into(),
                },
                "ERR_YT_CIPHER_UNSUPPORTED",
            ),
            (
                ExtractError::CipherFailed { transform: CipherTransform::N, reason: "x".into() },
                "ERR_YT_CIPHER_FAILED",
            ),
        ];
        for (error, code) in cases {
            assert_eq!(error.code(), code);
            assert!(error.to_string().contains(code));
        }
    }

    #[test]
    fn playability_statuses_map_to_dedicated_variants() {
        let response = |status, reason| serde_json::json!({"playabilityStatus": {"status": status, "reason": reason}});
        assert!(matches!(
            playability_error(&response(
                "LOGIN_REQUIRED",
                "This video may be inappropriate for some users. Confirm your age"
            )),
            Err(ExtractError::AgeRestricted { .. })
        ));
        assert!(matches!(
            playability_error(&response("LOGIN_REQUIRED", "Sign in to confirm you’re not a bot")),
            Err(ExtractError::BotCheck { .. })
        ));
        assert!(matches!(
            playability_error(&response("LOGIN_REQUIRED", "Sign in to watch this video")),
            Err(ExtractError::LoginRequired { .. })
        ));
        assert!(matches!(
            playability_error(&response("UNPLAYABLE", "This video is private.")),
            Err(ExtractError::PrivateVideo { .. })
        ));
        assert!(matches!(
            playability_error(&response("LIVE_STREAM_OFFLINE", "This live stream will begin soon")),
            Err(ExtractError::LiveOrPremiere { .. })
        ));
        assert!(matches!(
            playability_error(&response("UNPLAYABLE", "The video is unavailable in your country")),
            Err(ExtractError::Unavailable { .. })
        ));
    }

    #[test]
    fn live_detection_rejects_active_and_upcoming_but_not_replays() {
        let replay = serde_json::json!({"videoDetails": {"isLiveContent": true}});
        assert!(!response_indicates_live(&replay));
        let upcoming =
            serde_json::json!({"videoDetails": {"isLiveContent": true, "isUpcoming": true}});
        assert!(response_indicates_live(&upcoming));
        let active = serde_json::json!({
            "microformat": {"playerMicroformatRenderer": {"liveBroadcastDetails": {"isLiveNow": true}}}
        });
        assert!(response_indicates_live(&active));
    }

    #[test]
    fn pick_prefers_capped_mp4_h264_plus_m4a() {
        let formats = vec![
            video_format(136, 720, "mp4", CodecFamily::H264, 2_000_000),
            video_format(137, 1080, "mp4", CodecFamily::H264, 4_300_000),
            video_format(399, 1080, "mp4", CodecFamily::Av1, 2_400_000),
            video_format(248, 1080, "webm", CodecFamily::Vp9, 8_000_000),
            video_format(313, 2160, "mp4", CodecFamily::H264, 15_000_000),
            audio_format(140, "mp4", CodecFamily::Aac, 128_000),
            audio_format(251, "webm", CodecFamily::Opus, 160_000),
        ];
        let (video, audio) = pick(&formats, 1080).unwrap();
        assert_eq!(video.itag, 137);
        assert_eq!(audio.unwrap().itag, 140);
    }

    #[test]
    fn pick_falls_back_like_the_default_selector() {
        let opus_only = vec![
            video_format(137, 1080, "mp4", CodecFamily::H264, 4_300_000),
            audio_format(251, "webm", CodecFamily::Opus, 160_000),
        ];
        let (video, audio) = pick(&opus_only, 1080).unwrap();
        assert_eq!(video.itag, 137);
        assert_eq!(audio.unwrap().itag, 251);

        let progressive = vec![
            video_format(137, 1080, "mp4", CodecFamily::H264, 4_300_000),
            progressive_format(18, 360, 410_000),
        ];
        let (selected, audio) = pick(&progressive, 1080).unwrap();
        assert_eq!(selected.itag, 18);
        assert!(audio.is_none());

        let over_cap = vec![
            video_format(313, 2160, "mp4", CodecFamily::H264, 15_000_000),
            audio_format(140, "mp4", CodecFamily::Aac, 128_000),
        ];
        let (video, audio) = pick(&over_cap, 1080).unwrap();
        assert_eq!(video.itag, 313);
        assert_eq!(audio.unwrap().itag, 140);
    }

    #[tokio::test]
    async fn extraction_happy_path_uses_visionos_and_parses_formats_and_captions() {
        let _loopback = crate::util::allow_loopback_for_tests();
        let server = start_fixture(direct_routes()).await;
        let fetcher = ValidatedFetcher::with_endpoint_origin(&server.origin);
        let extraction = extract(VIDEO_ID, &fetcher, Some("SID=secret")).await.unwrap();

        assert_eq!(extraction.video_id, VIDEO_ID);
        assert_eq!(extraction.title, "Fixture Video");
        assert_eq!(extraction.author.as_deref(), Some("Fixture Channel"));
        assert_eq!(extraction.duration_seconds, Some(212));
        assert_eq!(extraction.thumbnails.len(), 2);
        assert_eq!(extraction.formats.len(), 6);
        assert_eq!(extraction.expires_in_seconds, Some(21600));
        assert_eq!(extraction.client_name, "VISIONOS");
        assert_eq!(extraction.web_client_version.as_deref(), Some("2.20260101.00.00"));
        assert!(extraction.hls_manifest_url.is_some());

        let h264 = extraction.formats.iter().find(|format| format.itag == 137).unwrap();
        assert_eq!(h264.codec_family, CodecFamily::H264);
        assert_eq!(h264.container, "mp4");
        assert_eq!(h264.height, Some(1080));
        assert_eq!(h264.content_length, Some(113_000_000));
        assert!(h264.is_video_only());
        assert!(!h264.was_ciphered);
        let progressive = extraction.formats.iter().find(|format| format.itag == 18).unwrap();
        assert!(progressive.progressive);
        assert!(progressive.has_audio);
        let aac = extraction.formats.iter().find(|format| format.itag == 140).unwrap();
        assert_eq!(aac.codec_family, CodecFamily::Aac);
        assert!(aac.is_audio_only());

        assert_eq!(extraction.captions.len(), 2);
        assert_eq!(extraction.captions[0].language_code, "tr");
        assert!(!extraction.captions[0].is_automatic);
        assert_eq!(extraction.captions[1].kind.as_deref(), Some("asr"));
        assert!(extraction.captions[1].is_automatic);
        assert_eq!(extraction.captions[1].name.as_deref(), Some("English (auto)"));

        let requests = server.recorded();
        assert_eq!(requests.len(), 3 - 1, "watch + player only; direct formats need no base.js");
        assert_eq!(requests[0].method, "GET");
        assert!(requests[0].target.starts_with("/watch?v="));
        assert_eq!(requests[0].header("cookie"), Some("SID=secret"));
        let player = &requests[1];
        assert_eq!(player.method, "POST");
        assert!(player.target.starts_with("/youtubei/v1/player?"));
        assert!(player.target.contains("key=fixture-key"));
        assert!(player.target.contains("prettyPrint=false"));
        assert_eq!(player.header("x-youtube-client-name"), Some("101"));
        assert_eq!(player.header("x-youtube-client-version"), Some("1.02"));
        assert_eq!(player.header("x-goog-visitor-id"), Some("visitor-data"));
        assert_eq!(player.header("origin"), Some("https://www.youtube.com"));
        assert_eq!(player.header("content-type"), Some("application/json"));
        let body: Value = serde_json::from_str(&player.body).unwrap();
        assert_eq!(body["videoId"], VIDEO_ID);
        assert_eq!(body["contentCheckOk"], true);
        assert_eq!(body["racyCheckOk"], true);
        assert_eq!(body["context"]["client"]["clientName"], "VISIONOS");
        assert_eq!(body["context"]["client"]["clientVersion"], "1.02");
        assert_eq!(body["context"]["client"]["deviceModel"], "RealityDevice17,1");
        assert_eq!(body["context"]["client"]["hl"], "en");
        assert_eq!(body["context"]["client"]["timeZone"], "UTC");
        assert_eq!(body["context"]["client"]["utcOffsetMinutes"], 0);
        assert_eq!(body["playbackContext"]["contentPlaybackContext"]["signatureTimestamp"], 23_123);
    }

    #[tokio::test]
    async fn ciphered_format_is_deciphered_and_n_is_transformed() {
        let _loopback = crate::util::allow_loopback_for_tests();
        let server = start_fixture(vec![
            ("/watch", response(200, "text/html", WATCH_PAGE)),
            ("/youtubei/v1/player", response(200, "application/json", ciphered_player_response())),
            ("/s/player/fixture01/base.js", response(200, "text/javascript", CLASSIC_PLAYER_JS)),
        ])
        .await;
        let fetcher = ValidatedFetcher::with_endpoint_origin(&server.origin);
        let extraction = extract(VIDEO_ID, &fetcher, None).await.unwrap();

        assert_eq!(extraction.formats.len(), 1);
        let format = &extraction.formats[0];
        assert!(format.was_ciphered);
        assert_eq!(
            format.url,
            "https://rr9---sn-cipher.googlevideo.com/videoplayback?id=ciphered&itag=137&n=cfedgba&expire=1893456000&sig=rstqponmlkjihgaedcb"
        );
        assert_eq!(
            extraction.hls_manifest_url.as_deref(),
            Some("https://manifest.googlevideo.com/api/manifest/hls_variant/n/cfedgba/file/index.m3u8")
        );
        let requests = server.recorded();
        assert_eq!(requests.len(), 3);
        assert_eq!(requests[2].target, "/s/player/fixture01/base.js");
    }

    #[tokio::test]
    async fn unsupported_cipher_shape_is_an_explicit_error() {
        let _loopback = crate::util::allow_loopback_for_tests();
        let inner = "https://rr9---sn-cipher.googlevideo.com/videoplayback?id=ciphered&itag=137";
        let cipher = format!("s=abcdefghijklmnopqrst&sp=sig&url={}", form_encode(inner));
        let player = serde_json::json!({
            "playabilityStatus": {"status": "OK"},
            "videoDetails": {"videoId": VIDEO_ID, "title": "Cipher Fixture", "lengthSeconds": "60"},
            "streamingData": {"adaptiveFormats": [{
                "itag": 137,
                "signatureCipher": cipher,
                "mimeType": "video/mp4; codecs=\"avc1\"",
                "width": 1920,
                "height": 1080
            }]}
        })
        .to_string();
        let result = extract_with_routes(vec![
            ("/watch", response(200, "text/html", WATCH_PAGE)),
            ("/youtubei/v1/player", response(200, "application/json", player)),
            (
                "/s/player/fixture01/base.js",
                response(200, "text/javascript", UNSUPPORTED_PLAYER_JS),
            ),
        ])
        .await;
        assert!(matches!(
            result,
            Err(ExtractError::CipherUnsupported { transform: CipherTransform::Signature, .. })
        ));
    }

    #[tokio::test]
    async fn http_malformed_and_no_format_failures_are_distinct() {
        let _loopback = crate::util::allow_loopback_for_tests();
        let http_result = extract_with_routes(vec![
            ("/watch", response(200, "text/html", WATCH_PAGE)),
            ("/youtubei/v1/player", response(500, "application/json", "{}")),
        ])
        .await;
        assert!(matches!(
            http_result,
            Err(ExtractError::Http { status: 500, context: "player API" })
        ));

        let malformed = extract_with_routes(vec![
            ("/watch", response(200, "text/html", WATCH_PAGE)),
            ("/youtubei/v1/player", response(200, "application/json", "not json")),
        ])
        .await;
        assert!(matches!(malformed, Err(ExtractError::MalformedResponse(_))));

        let no_formats = serde_json::json!({
            "playabilityStatus": {"status": "OK"},
            "videoDetails": {"videoId": VIDEO_ID, "title": "Empty", "lengthSeconds": "1"},
            "streamingData": {"formats": [], "adaptiveFormats": []}
        })
        .to_string();
        let empty = extract_with_routes(vec![
            ("/watch", response(200, "text/html", WATCH_PAGE)),
            ("/youtubei/v1/player", response(200, "application/json", no_formats)),
        ])
        .await;
        assert!(matches!(empty, Err(ExtractError::NoFormats)));
    }

    #[tokio::test]
    async fn connection_failure_is_network_error() {
        let _loopback = crate::util::allow_loopback_for_tests();
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        drop(listener);
        let fetcher = ValidatedFetcher::with_endpoint_origin(&format!("http://127.0.0.1:{port}"));
        let result = extract(VIDEO_ID, &fetcher, None).await;
        assert!(matches!(result, Err(ExtractError::Network(_))));
    }
}
