use std::path::{Component, Path, PathBuf};

/// Legacy insecure default — never accept as live token after first boot.
pub const LEGACY_DEFAULT_API_TOKEN: &str = "falcon-dm-local-v1";

pub fn is_hls_url(url: &str) -> bool {
    let lower = url.to_lowercase();
    if let Ok(parsed) = url::Url::parse(url) {
        let path = parsed.path().to_lowercase();
        if path.ends_with(".m3u8") || path.contains(".m3u8/") {
            return true;
        }
    }
    lower.contains(".m3u8")
}

/// Strip CR/LF from header values to prevent header injection.
pub fn sanitize_header_value(s: &str) -> String {
    s.chars()
        .filter(|c| *c != '\r' && *c != '\n')
        .collect()
}

fn is_blocked_download_host(host: &str) -> bool {
    let h = host.trim().trim_matches('[').trim_matches(']').to_lowercase();
    if h == "localhost" || h == "0.0.0.0" || h == "::1" {
        return true;
    }
    if h.starts_with("127.") || h.starts_with("10.") || h.starts_with("192.168.") {
        return true;
    }
    if let Some(rest) = h.strip_prefix("172.") {
        if let Some(second) = rest.split('.').next().and_then(|s| s.parse::<u8>().ok()) {
            if (16..=31).contains(&second) {
                return true;
            }
        }
    }
    if h.starts_with("169.254.") || h.starts_with("fc") || h.starts_with("fd") || h.starts_with("fe80:") {
        return true;
    }
    false
}

/// Allow only http(s) and magnet URLs. Block loopback/private hosts (SSRF).
pub fn validate_download_url(url: &str) -> Result<(), String> {
    let trimmed = url.trim();
    if trimmed.is_empty() {
        return Err("URL is empty".into());
    }
    if trimmed.starts_with("magnet:") {
        return Ok(());
    }
    let parsed = url::Url::parse(trimmed).map_err(|e| format!("Invalid URL: {e}"))?;
    match parsed.scheme() {
        "http" | "https" => {
            if let Some(host) = parsed.host_str() {
                if is_blocked_download_host(host) {
                    return Err("Private/loopback hosts are not allowed".into());
                }
            }
            Ok(())
        }
        other => Err(format!("Unsupported URL scheme: {other}")),
    }
}

/// Reject path traversal and absolute paths in filenames.
pub fn sanitize_filename(name: &str) -> String {
    let name = name.trim().replace('\\', "/");
    let base = name
        .rsplit('/')
        .next()
        .unwrap_or("download")
        .chars()
        .filter(|c| !matches!(c, '<' | '>' | ':' | '"' | '|' | '?' | '*' | '\0'))
        .collect::<String>();
    let base = base.trim().trim_start_matches('.');
    if base.is_empty() || base == ".." {
        "falcon_download".to_string()
    } else {
        base.chars().take(200).collect()
    }
}

/// Resolve save_path under allowed root; reject escape attempts.
pub fn resolve_save_dir(requested: &str, allowed_root: &Path) -> Result<PathBuf, String> {
    let expanded = expand_tilde(requested);
    let root = allowed_root
        .canonicalize()
        .unwrap_or_else(|_| allowed_root.to_path_buf());

    let candidate = if expanded.is_absolute() {
        expanded
    } else {
        root.join(&expanded)
    };

    // Ensure no `..` components remain after normalize
    let mut clean = PathBuf::new();
    for comp in candidate.components() {
        match comp {
            Component::ParentDir => {
                if !clean.pop() {
                    return Err("Invalid save path".into());
                }
            }
            Component::CurDir => {}
            other => clean.push(other.as_os_str()),
        }
    }

    // If path doesn't exist yet, canonicalize parent and append
    let verified = if clean.exists() {
        clean.canonicalize().map_err(|e| e.to_string())?
    } else {
        if let Some(parent) = clean.parent() {
            let parent_canon = if parent.exists() {
                parent.canonicalize().map_err(|e| e.to_string())?
            } else {
                std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
                parent.canonicalize().map_err(|e| e.to_string())?
            };
            parent_canon.join(clean.file_name().unwrap_or_default())
        } else {
            clean
        }
    };

    if !verified.starts_with(&root) {
        // Allow if under home Downloads as secondary root
        if let Some(dl) = dirs::download_dir() {
            let dl_root = dl.canonicalize().unwrap_or(dl);
            if verified.starts_with(&dl_root) {
                return Ok(verified);
            }
        }
        return Err("Save path must be under the Downloads folder".into());
    }
    Ok(verified)
}

pub fn expand_tilde(path: &str) -> PathBuf {
    if let Some(rest) = path.strip_prefix("~/") {
        if let Some(home) = dirs::home_dir() {
            return home.join(rest);
        }
    }
    if path == "~" {
        if let Some(home) = dirs::home_dir() {
            return home;
        }
    }
    PathBuf::from(path)
}

pub fn default_download_dir() -> PathBuf {
    dirs::download_dir().unwrap_or_else(|| {
        dirs::home_dir()
            .unwrap_or_else(|| PathBuf::from("."))
            .join("Downloads")
    })
}

pub fn app_data_dir() -> PathBuf {
    dirs::data_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join("com.falcondm.app")
}

pub fn full_file_path(save_path: &str, filename: &str) -> PathBuf {
    PathBuf::from(save_path).join(sanitize_filename(filename))
}

/// Reject completed downloads that are obvious error pages — any file type allowed.
pub fn validate_completed_file(path: &Path) -> Result<u64, String> {
    if !path.exists() {
        return Err("Downloaded file not found".into());
    }
    let meta = std::fs::metadata(path).map_err(|e| e.to_string())?;
    let size = meta.len();
    if size == 0 {
        return Err("Downloaded file is empty".into());
    }
    // Only sniff tiny payloads — likely HTML/JSON/protobuf error responses
    if size < 512 {
        let mut head = [0u8; 64];
        let read = {
            use std::io::Read;
            let mut f = std::fs::File::open(path).map_err(|e| e.to_string())?;
            f.read(&mut head).unwrap_or(0)
        };
        if read > 0 {
            let is_text = head[0] == b'<'
                || head[0] == b'{'
                || head.starts_with(b"HTTP")
                || head.starts_with(b"<!DOC");
            if is_text {
                return Err(
                    "Server returned an error page instead of a file".into(),
                );
            }
            // YouTube DASH/sabr error blobs
            if head.windows(4).any(|w| w == b"sabr") {
                return Err("Invalid stream response".into());
            }
        }
    }
    Ok(size)
}

/// Strip transient byte-range params WITHOUT re-encoding (preserves YouTube signatures).
pub fn normalize_media_url(url: &str) -> String {
    let mut out = url.to_string();
    for key in ["range", "rn", "rbuf", "sq", "ump", "alr", "keepalive", "cmo"] {
        let re = format!(r"([&?]){key}=[^&]*");
        if let Ok(pat) = regex_lite_remove(&out, &re) {
            out = pat;
        }
    }
    out = out.replace("&&", "&");
    if out.contains("?&") {
        out = out.replace("?&", "?");
    }
    if out.ends_with('&') || out.ends_with('?') {
        out.pop();
    }
    out
}

fn regex_lite_remove(input: &str, pattern: &str) -> Result<String, ()> {
    // Minimal remover: find `&key=` or `?key=` and cut until next `&`
    // pattern like ([&?])range=[^&]*
    let key = pattern
        .trim_start_matches("([&?])")
        .split('=')
        .next()
        .unwrap_or("")
        .trim_end_matches("=[^&]*");
    if key.is_empty() {
        return Ok(input.to_string());
    }
    let mut s = input.to_string();
    loop {
        let Some(pos) = s.find(&format!("&{key}=")).or_else(|| s.find(&format!("?{key}=")))
        else {
            break;
        };
        let value_start = s[pos..].find('=').map(|i| pos + i + 1).unwrap_or(s.len());
        let value_end = s[value_start..]
            .find('&')
            .map(|i| value_start + i)
            .unwrap_or(s.len());
        if s.as_bytes().get(pos) == Some(&b'?') {
            // ?key=value&rest → ?rest  OR ?key=value → (no query)
            if value_end < s.len() {
                s = format!("{}{}", &s[..pos + 1], &s[value_end + 1..]);
            } else {
                s = s[..pos].to_string();
            }
        } else {
            s = format!("{}{}", &s[..pos], &s[value_end..]);
        }
    }
    Ok(s)
}

/// Reject YouTube UI placeholders and tracking endpoints mistaken for media.
pub fn is_junk_media_url(url: &str) -> bool {
    let lower = url.to_lowercase();
    lower.contains("no_input.mp3")
        || lower.contains("/s/search/")
        || lower.contains("generate_204")
        || lower.contains("/ptracking")
        || lower.contains("/log_event")
        || lower.contains("doubleclick")
        || (lower.contains("youtube.com")
            && lower.contains(".mp3")
            && !lower.contains("videoplayback"))
}

pub fn is_youtube_watch_url(url: &str) -> bool {
    let Ok(u) = url::Url::parse(url) else {
        return false;
    };
    let host = u.host_str().unwrap_or("");
    if !(host.ends_with("youtube.com") || host == "youtu.be" || host.ends_with("youtube-nocookie.com"))
    {
        return false;
    }
    if host == "youtu.be" {
        return u.path_segments().and_then(|mut s| s.next()).is_some();
    }
    u.path().starts_with("/watch") || u.path().starts_with("/shorts/") || u.path().starts_with("/live/")
}

pub fn is_googlevideo_url(url: &str) -> bool {
    let lower = url.to_lowercase();
    lower.contains("googlevideo.com") || lower.contains("videoplayback")
}

/// Internal wire: `#falconfmt=<urlencoded yt-dlp -f>`. Prefer API field `format` — this fragment is storage/compat only.
pub fn split_falcon_format(url: &str) -> (String, Option<String>) {
    if let Some((base, frag)) = url.rsplit_once('#') {
        if let Some(enc) = frag.strip_prefix("falconfmt=") {
            let decoded = url::form_urlencoded::parse(format!("x={enc}").as_bytes())
                .next()
                .map(|(_, v)| v.into_owned())
                .unwrap_or_else(|| enc.to_string());
            if !decoded.is_empty() {
                return (base.to_string(), Some(decoded));
            }
        }
    }
    (url.to_string(), None)
}

/// Attach or replace yt-dlp format selector. `format` wins over existing fragment.
pub fn attach_falcon_format(url: &str, format: Option<&str>) -> String {
    let (base, existing) = split_falcon_format(url);
    let fmt = format
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(|s| s.to_string())
        .or(existing);
    match fmt {
        Some(f) => {
            let enc: String = url::form_urlencoded::byte_serialize(f.as_bytes()).collect();
            format!("{base}#falconfmt={enc}")
        }
        None => base,
    }
}

/// Prefer canonical watch URL for yt-dlp (CDN links 403 outside browser).
pub fn youtube_page_url_for_download(url: &str, referrer: Option<&str>) -> Option<String> {
    let (clean, _) = split_falcon_format(url);
    if is_youtube_watch_url(&clean) {
        return Some(url.to_string()); // keep falconfmt fragment if present
    }
    if let Some(r) = referrer {
        if is_youtube_watch_url(r) {
            return Some(r.to_string());
        }
    }
    // googlevideo: build watch URL from id= even without referrer
    if is_googlevideo_url(&clean) {
        if let Ok(u) = url::Url::parse(&clean) {
            for (k, v) in u.query_pairs() {
                if k == "id" && !v.is_empty() && v.len() >= 6 {
                    return Some(format!("https://www.youtube.com/watch?v={v}"));
                }
            }
        }
        if let Some(r) = referrer.filter(|s| s.contains("youtube.com") || s.contains("youtu.be")) {
            return Some(r.to_string());
        }
    }
    None
}

/// Pull a filename segment from URL path or query.
pub fn infer_filename_from_url(url: &str) -> Option<String> {
    let parsed = url::Url::parse(url).ok()?;
    for (k, v) in parsed.query_pairs() {
        if k == "filename" || k == "download" {
            let name = v.trim();
            if !name.is_empty() {
                return Some(name.to_string());
            }
        }
    }
    let segment = parsed
        .path()
        .rsplit('/')
        .next()
        .unwrap_or("")
        .split('?')
        .next()
        .unwrap_or("");
    if !segment.is_empty() && segment != "/" && segment.contains('.') {
        return Some(segment.to_string());
    }
    None
}

/// Guess extension from URL hints (mime=, path suffix).
pub fn guess_extension_from_url(url: &str) -> Option<String> {
    let lower = url.to_lowercase();
    if lower.contains(".m3u8") || lower.contains("/manifest/") {
        return Some("mp4".into());
    }
    if let Some(m) = lower.find("mime=") {
        let mime = &lower[m + 5..];
        let mime = mime.split(&['&', ';'][..]).next().unwrap_or("");
        let ext = match mime {
            "video%2Fmp4" | "video/mp4" => "mp4",
            "video%2Fwebm" | "video/webm" => "webm",
            "audio%2Fmp4" | "audio/mp4" | "audio%2Fm4a" | "audio/m4a" => "m4a",
            "audio%2Fwebm" | "audio/webm" => "webm",
            "audio%2Fmpeg" | "audio/mpeg" => "mp3",
            "application%2Fzip" | "application/zip" => "zip",
            "application%2Fpdf" | "application/pdf" => "pdf",
            "application%2Foctet-stream" | "application/octet-stream" => return None,
            _ => return None,
        };
        return Some(ext.into());
    }
    for ext in [
        "mp4", "webm", "mkv", "avi", "mov", "m4a", "mp3", "flac", "ogg", "wav", "zip", "rar",
        "7z", "pdf", "exe", "dmg", "pkg", "iso", "torrent", "png", "jpg", "jpeg", "gif",
    ] {
        if lower.contains(&format!(".{ext}")) {
            return Some(ext.into());
        }
    }
    None
}

/// Resolve final filename: explicit > URL > title+ext > generic.
pub fn resolve_download_filename(
    url: &str,
    explicit: Option<&str>,
    title: Option<&str>,
    force_hls_mp4: bool,
) -> String {
    if let Some(f) = explicit.map(str::trim).filter(|s| !s.is_empty()) {
        return sanitize_filename(f);
    }
    if let Some(inferred) = infer_filename_from_url(url) {
        return sanitize_filename(&inferred);
    }
    let ext = if force_hls_mp4 || is_hls_url(url) {
        "mp4".to_string()
    } else {
        guess_extension_from_url(url).unwrap_or_else(|| "bin".into())
    };
    if let Some(t) = title.map(str::trim).filter(|s| !s.is_empty()) {
        let clean: String = t
            .chars()
            .filter(|c| c.is_alphanumeric() || c.is_whitespace() || *c == '-' || *c == '_')
            .collect();
        let clean = clean.trim();
        if !clean.is_empty() {
            return sanitize_filename(&format!("{clean}.{ext}"));
        }
    }
    sanitize_filename(&format!("falcon_download.{ext}"))
}

/// Ensure path is under Downloads or configured root before opening.
pub fn validate_open_path(path: &str) -> Result<PathBuf, String> {
    let expanded = expand_tilde(path);
    let candidate = if expanded.exists() {
        expanded.canonicalize().map_err(|e| e.to_string())?
    } else if let Some(parent) = expanded.parent() {
        let parent_c = if parent.exists() {
            parent.canonicalize().map_err(|e| e.to_string())?
        } else {
            return Err("Path does not exist".into());
        };
        parent_c.join(expanded.file_name().unwrap_or_default())
    } else {
        return Err("Invalid path".into());
    };

    let mut allowed = vec![default_download_dir()];
    if let Some(home) = dirs::home_dir() {
        allowed.push(home.join("Downloads"));
        allowed.push(home.join("Movies"));
        allowed.push(home.join("Music"));
        allowed.push(home.join("Documents"));
    }
    allowed.push(app_data_dir());

    for root in allowed {
        let root_c = root.canonicalize().unwrap_or(root);
        if candidate.starts_with(&root_c) {
            return Ok(candidate);
        }
    }
    Err("Path is outside allowed directories".into())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_sanitize_filename_blocks_traversal() {
        assert_eq!(sanitize_filename("../../etc/passwd"), "passwd");
        assert_eq!(sanitize_filename("a/b/c.zip"), "c.zip");
        assert!(!sanitize_filename("foo\0bar").contains('\0'));
    }

    #[test]
    fn test_validate_url() {
        assert!(validate_download_url("https://example.com/a").is_ok());
        assert!(validate_download_url("magnet:?xt=urn:btih:abc").is_ok());
        assert!(validate_download_url("file:///etc/passwd").is_err());
        assert!(validate_download_url("javascript:alert(1)").is_err());
        assert!(validate_download_url("http://127.0.0.1/secret").is_err());
        assert!(validate_download_url("http://localhost/x").is_err());
        assert!(validate_download_url("http://192.168.1.1/a").is_err());
    }

    #[test]
    fn test_youtube_page_url_from_googlevideo_id() {
        let cdn = "https://rr1---sn-abc.googlevideo.com/videoplayback?id=5-u7nkMiwtQ&mime=video%2Fmp4";
        let watch = youtube_page_url_for_download(cdn, None).unwrap();
        assert!(watch.contains("watch?v=5-u7nkMiwtQ"));
    }

    #[test]
    fn test_split_falcon_format() {
        let (base, fmt) = split_falcon_format(
            "https://www.youtube.com/watch?v=abc#falconfmt=best%5Bheight%3C%3D720%5D",
        );
        assert_eq!(base, "https://www.youtube.com/watch?v=abc");
        assert!(fmt.unwrap().contains("height"));
    }

    #[test]
    fn test_attach_falcon_format() {
        let u = attach_falcon_format("https://www.youtube.com/watch?v=abc", Some("best[height<=720]"));
        let (base, fmt) = split_falcon_format(&u);
        assert_eq!(base, "https://www.youtube.com/watch?v=abc");
        assert_eq!(fmt.as_deref(), Some("best[height<=720]"));
        assert_eq!(
            attach_falcon_format("https://x.com/a#falconfmt=old", Some("new")),
            attach_falcon_format("https://x.com/a", Some("new"))
        );
    }

    #[test]
    fn test_sanitize_header() {
        assert_eq!(sanitize_header_value("a\r\nX-Inject: 1"), "aX-Inject: 1");
    }

    #[test]
    fn test_is_hls_url() {
        assert!(is_hls_url("https://cdn.example.com/live/index.m3u8"));
        assert!(is_hls_url("https://cdn.example.com/a.m3u8?token=1"));
        assert!(!is_hls_url("https://cdn.example.com/video.mp4"));
    }

    #[test]
    fn test_validate_open_path_rejects_etc() {
        let res = validate_open_path("/etc/passwd");
        assert!(res.is_err());
    }

    #[test]
    fn test_junk_and_normalize_media_url() {
        assert!(is_junk_media_url(
            "https://www.youtube.com/s/search/audio/no_input.mp3"
        ));
        let n = normalize_media_url(
            "https://rr1.googlevideo.com/videoplayback?itag=18&range=0-100&sq=1",
        );
        assert!(!n.contains("range="));
        assert!(!n.contains("sq="));
        assert!(n.contains("itag=18"));
    }

    #[test]
    fn test_infer_filename_from_url() {
        assert_eq!(
            infer_filename_from_url("https://cdn.example.com/files/report.pdf?token=1"),
            Some("report.pdf".into())
        );
        assert_eq!(
            infer_filename_from_url("https://x.com/dl?filename=archive.zip"),
            Some("archive.zip".into())
        );
    }

    #[test]
    fn test_resolve_download_filename_no_mp4_force() {
        let name = resolve_download_filename(
            "https://cdn.example.com/movie.webm",
            None,
            Some("My Video"),
            false,
        );
        assert!(name.ends_with(".webm"));
    }

    #[test]
    fn test_validate_completed_file_rejects_tiny() {
        let dir = std::env::temp_dir().join("falcon_test_tiny");
        let _ = std::fs::create_dir_all(&dir);
        let p = dir.join("error.html");
        std::fs::write(&p, b"<html>error</html>").unwrap();
        assert!(validate_completed_file(&p).is_err());
        let _ = std::fs::remove_file(&p);
    }
}
