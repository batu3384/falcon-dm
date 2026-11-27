mod commands;
pub mod download;
mod lifecycle;
mod local_api;
pub mod log_buffer;
pub mod pairing;
pub mod settings;
pub mod storage;
pub mod util;

pub use lifecycle::run;

use chrono::Utc;
use download::queue::{QueueManager, StopReason};
use pairing::PairingState;
use serde::Deserialize;
use settings::Settings;
use storage::models::{Download, DownloadCategory, DownloadFilter, DownloadStatus};
use storage::Database;
use storage::InsertDownloadResult;
use tauri::{AppHandle, Emitter, Manager};
use tauri_plugin_notification::NotificationExt;
use util::{
    default_download_dir, is_googlevideo_url, is_hls_url, is_junk_media_url, is_youtube_host,
    lock_or_recover, normalize_media_url, resolve_download_filename, resolve_download_target,
    resolve_save_dir, sanitize_filename, sanitize_header_value, unique_download_filename,
    validate_enqueue_url, write_file_exclusive, LEGACY_DEFAULT_API_TOKEN,
};

use axum::http::{HeaderMap, StatusCode};
use std::collections::VecDeque;
use std::sync::{Arc, Mutex};
use std::time::Instant;
use subtle::ConstantTimeEq;

/// Constant-time string comparison to prevent timing attacks on the API token.
/// Falls back to a non-equal result (without short-circuit) if lengths differ.
pub(crate) fn token_matches(provided: &str, expected: &str) -> bool {
    let p = provided.as_bytes();
    let e = expected.as_bytes();
    if p.len() != e.len() {
        // Still do a comparison to keep timing roughly uniform.
        let _ = p.ct_eq(p);
        return false;
    }
    p.ct_eq(e).into()
}

pub(crate) fn extension_id_from_origin(origin: &str) -> Option<&str> {
    origin.strip_prefix("chrome-extension://").or_else(|| origin.strip_prefix("edge-extension://"))
}

pub(crate) fn is_valid_extension_id(id: &str) -> bool {
    id.len() == 32 && id.bytes().all(|byte| (b'a'..=b'p').contains(&byte))
}

/// Canonical form of an HTTP(S) URL for storage: lowercase scheme and host,
/// no default port, non-empty path. Dedupe and resume-state compare URLs as
/// strings, so one spelling must win. Queries and fragments pass through
/// byte-for-byte (YouTube signatures); anything unparseable or non-HTTP(S)
/// (blob uploads) is kept as-is.
pub(crate) fn canonical_store_url(raw: &str) -> String {
    match url::Url::parse(raw) {
        Ok(parsed) if matches!(parsed.scheme(), "http" | "https") => parsed.into(),
        _ => raw.to_string(),
    }
}

pub(crate) fn parse_extension_id(raw: &str) -> Result<&str, String> {
    let id = raw.trim();
    if is_valid_extension_id(id) {
        Ok(id)
    } else {
        Err("invalid extension id".into())
    }
}

pub(crate) fn cookie_url_matches_download(download_url: &str, cookie_url: Option<&str>) -> bool {
    let Some(cookie_url) = cookie_url else {
        return false;
    };
    let Ok(download) = url::Url::parse(download_url) else {
        return false;
    };
    let Ok(cookie_source) = url::Url::parse(cookie_url) else {
        return false;
    };
    if matches!(download.scheme(), "https")
        && download.scheme() == cookie_source.scheme()
        && download.host_str().map(str::to_ascii_lowercase)
            == cookie_source.host_str().map(str::to_ascii_lowercase)
        && download.port_or_known_default() == cookie_source.port_or_known_default()
    {
        return true;
    }
    // YouTube CDN: browser sends youtube.com cookies for googlevideo streams.
    if is_googlevideo_url(download_url)
        && download.scheme() == "https"
        && cookie_source.scheme() == "https"
    {
        if let Some(host) = cookie_source.host_str() {
            if is_youtube_host(host) {
                return true;
            }
        }
    }
    false
}

/// Push the authoritative row to the UI after any status or metadata change.
/// Progress events only carry bytes; this is what moves a row between states.
pub(crate) fn emit_row(app: &AppHandle, db: &Database, id: i64) {
    if let Ok(row) = db.get_download(id) {
        let _ = app.emit("download-updated", &row);
    }
}

fn is_turkish(lang: &str) -> bool {
    lang.trim().split(['-', '_', '.']).next().is_some_and(|tag| tag.eq_ignore_ascii_case("tr"))
}

/// OS notification for a finished or failed download. Skipped while the window
/// is focused: the list already shows it.
pub(crate) fn notify_download(app: &AppHandle, db: &Database, id: i64, success: bool) {
    let window = app.get_webview_window("main");
    if window.as_ref().is_some_and(|w| w.is_focused().unwrap_or(false)) {
        return;
    }
    let Ok(row) = db.get_download(id) else {
        return;
    };
    let turkish = is_turkish(&lock_or_recover(&app.state::<AppState>().ui_lang));
    let title = match (success, turkish) {
        (true, true) => "İndirme tamamlandı",
        (true, false) => "Download complete",
        (false, true) => "İndirme başarısız",
        (false, false) => "Download failed",
    };
    let _ = app.notification().builder().title(title).body(&row.filename).show();
    if let Some(window) = window {
        let _ = window.request_user_attention(Some(tauri::UserAttentionType::Informational));
    }
}

/// Remove every temp artifact a download may have left, whichever engine ran it.
pub(crate) fn remove_download_artifacts(save_path: &str, filename: &str, id: i64) {
    download::http::remove_artifacts(save_path, filename, id);
    download::hls::remove_artifacts(id);
    download::ytdlp::remove_artifacts(save_path, id);
}

pub struct AppState {
    pub db: Database,
    pub queue: QueueManager,
    pub api_token: Arc<Mutex<String>>,
    pub settings: Arc<Mutex<Settings>>,
    pub rate_bucket: Arc<Mutex<VecDeque<Instant>>>,
    /// Extensions waiting for the user to compare codes and approve.
    pub pairing: Arc<Mutex<PairingState>>,
    /// UI language, for notifications and the tray menu.
    pub ui_lang: Arc<Mutex<String>>,
}

#[cfg(test)]
pub(crate) fn test_app_state(token: &str, ext_id: &str) -> AppState {
    let settings =
        Settings { allowed_extension_ids: vec![ext_id.to_string()], ..Default::default() };
    AppState {
        db: Database::in_memory().unwrap(),
        queue: QueueManager::new(),
        api_token: Arc::new(Mutex::new(token.to_string())),
        settings: Arc::new(Mutex::new(settings)),
        rate_bucket: Arc::new(Mutex::new(VecDeque::new())),
        pairing: Arc::new(Mutex::new(PairingState::default())),
        ui_lang: Arc::new(Mutex::new("en".into())),
    }
}

pub(crate) fn show_main_window(app: &AppHandle) {
    if let Some(window) = app.get_webview_window("main") {
        let _ = window.show();
        let _ = window.unminimize();
        let _ = window.set_focus();
    }
}

#[derive(Clone, Deserialize, Default)]
pub(crate) struct ExternalDownloadPayload {
    pub url: String,
    pub filename: Option<String>,
    pub referrer: Option<String>,
    pub user_agent: Option<String>,
    pub cookies: Option<String>,
    pub cookie_url: Option<String>,
    pub title: Option<String>,
    /// yt-dlp `-f` selector (preferred over `#falconfmt=` fragment).
    pub format: Option<String>,
    pub save_path: Option<String>,
}

/// A selector with no video stream (`bestaudio[ext=m4a]/bestaudio`).
pub(crate) fn is_audio_only_format(format: &str) -> bool {
    let format = format.trim().to_ascii_lowercase();
    !format.is_empty()
        && format.split('/').all(|alt| {
            let alt = alt.trim();
            (alt.starts_with("bestaudio") || alt.starts_with("ba")) && !alt.contains('+')
        })
}

fn with_extension(filename: &str, ext: &str) -> String {
    let stem = std::path::Path::new(filename)
        .file_stem()
        .and_then(|s| s.to_str())
        .filter(|s| !s.is_empty())
        .unwrap_or("download");
    format!("{stem}.{ext}")
}

pub(crate) async fn enqueue_download(
    app: &AppHandle,
    payload: ExternalDownloadPayload,
) -> Result<i64, String> {
    // No DNS here: a URL added while offline must queue and retry later.
    if let Err(reason) = validate_enqueue_url(&payload.url) {
        return Err(if reason == "ERR_UNSUPPORTED_MAGNET" { reason } else { "invalid url".into() });
    }
    if is_junk_media_url(&payload.url) {
        return Err("not a real media url".into());
    }

    let watch_url = util::youtube_page_url_for_download(&payload.url, payload.referrer.as_deref());
    let base = watch_url.clone().unwrap_or_else(|| normalize_media_url(payload.url.trim()));
    // One spelling everywhere: dedupe, resume state and routing compare strings.
    let base = canonical_store_url(&base);
    let format = payload.format.as_deref().or(if watch_url.is_some() {
        Some(download::ytdlp::DEFAULT_VIDEO_FORMAT)
    } else {
        None
    });
    let url = util::attach_falcon_format(&base, format);
    let force_hls = is_hls_url(&url);
    let mut filename = resolve_download_filename(
        &url,
        payload.filename.as_deref(),
        payload.title.as_deref(),
        force_hls,
    );
    // yt-dlp decides the container. Give the row the extension it will end up with.
    let via_ytdlp = download::queue::route_queued_download(&url, payload.referrer.as_deref())
        == download::queue::DownloadRoute::YtDlp;
    if via_ytdlp {
        let (_, selector) = util::split_falcon_format(&url);
        let audio = selector.as_deref().is_some_and(is_audio_only_format);
        let current = filename.rsplit('.').next().unwrap_or("");
        if audio && !download::ytdlp::is_audio_extension(current) {
            filename = with_extension(&filename, "m4a");
        } else if !audio && !filename.to_lowercase().ends_with(".mp4") {
            filename = with_extension(&filename, "mp4");
        }
    }
    let category = DownloadCategory::from_filename(&filename);

    // ponytail: apply a per-site profile if one matches. Profile fields only fill
    // GAPS — an explicit value from the payload (the user/extension's choice)
    // always wins. save_subdir overrides the category folder when set.
    let state = app.state::<AppState>();
    let settings = current_settings(&state);
    let profile = settings.profile_for_url(&url);
    let save_subdir = profile.and_then(|p| p.save_subdir.clone());
    let requested_save = payload
        .save_path
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .or(save_subdir.as_deref());
    let save_path = resolve_download_save_path_with(&settings, requested_save, &category)?;
    let filename = unique_download_filename(&save_path, &filename)?;

    let prof_referrer = profile.and_then(|p| p.referrer.clone());
    let prof_ua = profile.and_then(|p| p.user_agent.clone());
    let prof_cookies = profile.and_then(|p| p.cookies.clone());
    let referrer = payload.referrer.as_deref().or(prof_referrer.as_deref());

    let mut dl = Download {
        id: None,
        url: url.clone(),
        filename,
        save_path,
        total_size: 0,
        downloaded_size: 0,
        status: DownloadStatus::Queued,
        category,
        speed: 0.0,
        segments: settings.max_connections_per_server,
        priority: 1,
        created_at: Utc::now().to_rfc3339(),
        completed_at: None,
        error_message: None,
        referrer: resolve_media_referer(&url, referrer),
        user_agent: payload.user_agent.clone().or(prof_ua).map(|s| sanitize_header_value(&s)),
        cookies: payload
            .cookies
            .filter(|c| !c.trim().is_empty())
            .filter(|_| cookie_url_matches_download(&url, payload.cookie_url.as_deref()))
            .or(prof_cookies)
            .map(|s| sanitize_header_value(&s)),
        archived: false,
        attempts: 0,
        retry_at: None,
    };

    let insert_result = state.db.insert_download_deduped(&mut dl).map_err(|e| e.to_string())?;
    let id = insert_result.id();
    if matches!(insert_result, InsertDownloadResult::Created(_)) {
        dl.id = Some(id);
        let _ = app.emit("download-added", &dl);
    }
    Ok(id)
}

const MAX_UPLOAD_BYTES: usize = 32 * 1024 * 1024;
// ponytail: base64 expands ~4/3 — reject oversized payloads before decode allocates.
const MAX_UPLOAD_B64_LEN: usize = (MAX_UPLOAD_BYTES / 3).saturating_mul(4) + 8;

pub(crate) fn validate_upload_b64_len(b64_len: usize) -> Result<(), String> {
    if b64_len > MAX_UPLOAD_B64_LEN {
        return Err(format!(
            "upload payload too large (max {} MB)",
            MAX_UPLOAD_BYTES / 1024 / 1024
        ));
    }
    Ok(())
}

pub(crate) fn validate_upload_file(filename: &str, byte_len: usize) -> Result<(), String> {
    if byte_len == 0 {
        return Err("empty upload".into());
    }
    if byte_len > MAX_UPLOAD_BYTES {
        return Err(format!("upload too large (max {} MB)", MAX_UPLOAD_BYTES / 1024 / 1024));
    }
    if filename.trim().is_empty() {
        return Err("invalid filename".into());
    }
    if sanitize_filename(filename.trim()).is_empty() {
        return Err("invalid filename".into());
    }
    Ok(())
}

/// Save bytes uploaded from the browser extension (blob: capture) as a completed download.
pub(crate) async fn ingest_uploaded_file(
    app: &AppHandle,
    filename: &str,
    bytes: Vec<u8>,
    source_page: Option<String>,
) -> Result<i64, String> {
    validate_upload_file(filename, bytes.len())?;
    let safe = sanitize_filename(filename.trim());
    let category = DownloadCategory::from_filename(&safe);
    let state = app.state::<AppState>();
    let settings = current_settings(&state);
    let save_path = resolve_download_save_path_with(&settings, None, &category)?;
    let size = bytes.len() as u64;
    let now = Utc::now().to_rfc3339();
    let mut dl = Download {
        id: None,
        url: source_page.unwrap_or_else(|| "blob:".into()),
        filename: safe,
        save_path: save_path.clone(),
        total_size: size,
        downloaded_size: size,
        status: DownloadStatus::Completed,
        category,
        speed: 0.0,
        segments: 1,
        priority: 1,
        created_at: now.clone(),
        completed_at: Some(now),
        error_message: None,
        referrer: None,
        user_agent: None,
        cookies: None,
        archived: false,
        attempts: 0,
        retry_at: None,
    };
    let insert_result = state.db.insert_download_deduped(&mut dl).map_err(|e| e.to_string())?;
    let id = insert_result.id();
    if matches!(insert_result, InsertDownloadResult::Existing(_)) {
        return Ok(id);
    }
    let dest = resolve_download_target(&save_path, &dl.filename)?;
    let write = tokio::task::spawn_blocking(move || write_file_exclusive(&dest, &bytes))
        .await
        .map_err(|e| e.to_string())
        .and_then(|inner| inner);
    if let Err(e) = write {
        let _ = state.db.delete_download(id);
        return Err(format!("Could not save upload: {e}"));
    }
    dl.id = Some(id);
    let _ = app.emit("download-added", &dl);
    Ok(id)
}

/// Pause every running or queued download (toolbar, tray, command palette).
pub(crate) async fn pause_all(app: &AppHandle) -> usize {
    let state = app.state::<AppState>();
    let rows = state
        .db
        .get_downloads(&DownloadFilter {
            statuses: Some(vec![
                DownloadStatus::Queued,
                DownloadStatus::Downloading,
                DownloadStatus::Merging,
            ]),
            ..Default::default()
        })
        .unwrap_or_default();
    // Signal all workers first so they stop in parallel, then settle each row.
    for row in &rows {
        if let Some(id) = row.id {
            state.queue.request_stop(id, StopReason::Pause);
        }
    }
    let mut paused = 0;
    for row in rows {
        if let Some(id) = row.id {
            if state.queue.pause(&state.db, app, id).await {
                paused += 1;
            }
        }
    }
    paused
}

/// Requeue every paused or failed download.
pub(crate) fn resume_all(app: &AppHandle) -> usize {
    let state = app.state::<AppState>();
    let rows = state
        .db
        .get_downloads(&DownloadFilter {
            statuses: Some(vec![DownloadStatus::Paused, DownloadStatus::Failed]),
            ..Default::default()
        })
        .unwrap_or_default();
    let mut resumed = 0;
    for row in rows {
        let Some(id) = row.id else { continue };
        if state
            .db
            .resume_if_current(id, &[DownloadStatus::Paused, DownloadStatus::Failed])
            .unwrap_or(false)
        {
            emit_row(app, &state.db, id);
            resumed += 1;
        }
    }
    resumed
}

pub(crate) fn handle_deep_link_url(app: &AppHandle, raw: &str) {
    show_main_window(app);
    let Ok(parsed) = url::Url::parse(raw) else {
        return;
    };
    let host = parsed.host_str().unwrap_or("").to_lowercase();
    // Only wake / empty host — never enqueue via deep link (token-in-URL is a secret leak).
    // Cold-start downloads: extension must wake → wait healthy → HTTP /api/*.
    if host == "wake" || host.is_empty() {
        return;
    }
    log::warn!("deep-link host '{host}' ignored — downloads only via authenticated HTTP API");
}

pub(crate) fn current_settings(state: &AppState) -> Settings {
    lock_or_recover(&state.settings).clone()
}

pub(crate) fn resolve_download_save_path(
    state: &AppState,
    requested: Option<&str>,
    category: &DownloadCategory,
) -> Result<String, String> {
    resolve_download_save_path_with(&current_settings(state), requested, category)
}

fn resolve_download_save_path_with(
    settings: &Settings,
    requested: Option<&str>,
    category: &DownloadCategory,
) -> Result<String, String> {
    let fallback = default_download_dir();
    let _ = std::fs::create_dir_all(&fallback);

    let root = if settings.default_download_path.trim().is_empty() {
        fallback.clone()
    } else {
        let expanded = util::expand_tilde(&settings.default_download_path);
        if util::is_unsafe_download_root(&expanded) {
            fallback.clone()
        } else if std::fs::create_dir_all(&expanded).is_ok() {
            expanded
        } else {
            fallback.clone()
        }
    };

    let preferred = requested
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .or_else(|| settings.path_for_category(category.as_str()).filter(|p| !p.trim().is_empty()))
        .unwrap_or_else(|| root.to_string_lossy().to_string());

    match resolve_save_dir(&preferred, &root) {
        Ok(resolved) => {
            std::fs::create_dir_all(&resolved).map_err(|e| e.to_string())?;
            Ok(resolved.to_string_lossy().to_string())
        }
        Err(e) => {
            // Only soft-fallback when path is invalid-but-benign (missing parent), not traversal
            if e.contains("outside allowed") || e.contains("Invalid save path") {
                return Err(e);
            }
            log::warn!("save path rejected ({e}); falling back to Downloads");
            let fb = fallback;
            std::fs::create_dir_all(&fb).map_err(|e| e.to_string())?;
            Ok(fb.to_string_lossy().to_string())
        }
    }
}

pub(crate) fn check_api_token(
    headers: &HeaderMap,
    state: &AppState,
    origin: Option<&str>,
) -> Result<(), StatusCode> {
    let expected = lock_or_recover(&state.api_token).clone();
    if expected.trim().is_empty() || expected == LEGACY_DEFAULT_API_TOKEN {
        return Err(StatusCode::UNAUTHORIZED);
    }
    let provided = headers.get("x-falcon-token").and_then(|v| v.to_str().ok()).unwrap_or("");
    if !token_matches(provided, &expected) {
        return Err(StatusCode::UNAUTHORIZED);
    }

    // Origin required — blocks bare curl with stolen token (must forge extension Origin + allowlist).
    let origin = origin.filter(|s| !s.is_empty()).ok_or(StatusCode::FORBIDDEN)?;
    let ext_id = extension_id_from_origin(origin).ok_or(StatusCode::FORBIDDEN)?;
    if !is_valid_extension_id(ext_id) {
        return Err(StatusCode::FORBIDDEN);
    }
    let settings = lock_or_recover(&state.settings);
    if !settings.allowed_extension_ids.iter().any(|x| x == ext_id) {
        return Err(StatusCode::FORBIDDEN);
    }
    Ok(())
}

fn resolve_media_referer(url: &str, referer: Option<&str>) -> Option<String> {
    let r = referer.filter(|s| !s.is_empty());
    let resolved = if is_googlevideo_url(url) || util::is_youtube_watch_url(url) {
        r.or(Some("https://www.youtube.com/"))
    } else {
        r
    };
    resolved.map(sanitize_header_value)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extension_origin_must_contain_a_valid_id() {
        assert_eq!(
            extension_id_from_origin("chrome-extension://aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"),
            Some("aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa")
        );
        assert!(extension_id_from_origin("https://example.com").is_none());
        assert!(
            extension_id_from_origin("moz-extension://aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa").is_none()
        );
        assert!(!is_valid_extension_id("short"));
        assert!(!is_valid_extension_id("zzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzz"));
        assert!(parse_extension_id(" aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa ").is_ok());
        assert!(parse_extension_id("zzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzz").is_err());
    }

    #[test]
    fn extension_cookie_url_must_match_download_origin() {
        assert!(cookie_url_matches_download(
            "https://cdn.example.com/video.mp4",
            Some("https://cdn.example.com/video.mp4")
        ));
        assert!(!cookie_url_matches_download(
            "https://cdn.example.com/video.mp4",
            Some("https://page.example.com/watch")
        ));
        assert!(!cookie_url_matches_download(
            "http://cdn.example.com/video.mp4",
            Some("https://cdn.example.com/video.mp4")
        ));
        assert!(!cookie_url_matches_download(
            "http://cdn.example.com/video.mp4",
            Some("http://cdn.example.com/video.mp4")
        ));
        assert!(cookie_url_matches_download(
            "https://rr3---sn-abc.googlevideo.com/videoplayback?id=xyz",
            Some("https://www.youtube.com/watch?v=abc")
        ));
        assert!(!cookie_url_matches_download(
            "https://rr3---sn-abc.googlevideo.com/videoplayback?id=xyz",
            Some("https://vimeo.com/123")
        ));
        assert!(!cookie_url_matches_download(
            "https://rr3---sn-abc.googlevideo.com/videoplayback?id=xyz",
            Some("https://notyoutube.com/watch")
        ));
        assert!(!cookie_url_matches_download(
            "https://rr3---sn-abc.googlevideo.com/videoplayback?id=xyz",
            Some("http://www.youtube.com/watch?v=abc")
        ));
    }

    #[test]
    fn upload_validation_rejects_oversized_b64_and_bytes() {
        assert!(validate_upload_b64_len(MAX_UPLOAD_B64_LEN + 1).is_err());
        assert!(validate_upload_file("video.mp4", MAX_UPLOAD_BYTES + 1).is_err());
        assert!(validate_upload_file("", 1).is_err());
        assert!(validate_upload_file("ok.mp4", 1024).is_ok());
    }

    #[test]
    fn audio_only_selectors_are_recognized() {
        assert!(is_audio_only_format("bestaudio[ext=m4a]/bestaudio"));
        assert!(is_audio_only_format("ba"));
        assert!(!is_audio_only_format("bestvideo[height<=720]+bestaudio/best[height<=720]"));
        assert!(!is_audio_only_format(download::ytdlp::DEFAULT_VIDEO_FORMAT));
        assert!(!is_audio_only_format("bestaudio/best"));
        assert!(!is_audio_only_format(""));
        assert_eq!(with_extension("My Song.webm", "m4a"), "My Song.m4a");
        assert_eq!(with_extension("noext", "mp4"), "noext.mp4");
    }

    #[test]
    fn canonical_store_url_normalizes_scheme_host_port_and_path() {
        assert_eq!(
            canonical_store_url("HTTPS://CDN.EXAMPLE.COM:443/a.bin"),
            "https://cdn.example.com/a.bin"
        );
        assert_eq!(canonical_store_url("https://cdn.example.com"), "https://cdn.example.com/");
        assert_eq!(canonical_store_url("http://example.com:80/x"), "http://example.com/x");
        // Query bytes and the falconfmt fragment survive verbatim.
        let signed = "https://cdn.example.com/v?sig=a%2Fb%3D&x=1#falconfmt=best";
        assert_eq!(canonical_store_url(signed), signed);
        // Non-HTTP(S) rows (blob uploads) and unparseable strings pass through.
        assert_eq!(canonical_store_url("blob:https://x/y"), "blob:https://x/y");
        assert_eq!(canonical_store_url("not a url"), "not a url");
    }

    #[test]
    fn turkish_locale_tags_are_detected() {
        assert!(is_turkish("tr"));
        assert!(is_turkish("tr-TR"));
        assert!(is_turkish("tr_TR.UTF-8"));
        assert!(!is_turkish("en"));
        assert!(!is_turkish("en_US"));
    }

    #[test]
    fn save_path_outside_root_is_rejected_not_redirected() {
        let settings = Settings::default();
        let requested = std::env::temp_dir().join(format!(
            "falcon-dm-outside-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or(0)
        ));
        let err = resolve_download_save_path_with(
            &settings,
            Some(requested.to_str().expect("utf8 temp path")),
            &DownloadCategory::Other,
        )
        .expect_err("outside-root save must error, not fall back to Downloads");
        assert!(err.contains("outside allowed") || err.contains("Invalid save path"), "{err}");
        assert!(!requested.exists());
    }

    #[test]
    fn artifacts_of_every_engine_are_removed() {
        let dir = std::env::temp_dir().join(format!("falcon-artifacts-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let save = dir.to_string_lossy().to_string();
        let id = 1_000_000_000_000i64 + i64::from(std::process::id());
        let part = download::http::part_path_for(&save, "a.bin", id).unwrap();
        let state = download::http::state_path_for(&save, "a.bin", id).unwrap();
        std::fs::write(&part, b"partial").unwrap();
        std::fs::write(&state, b"{}").unwrap();
        let run_dir = download::ytdlp::run_dir_for(&save, id);
        std::fs::create_dir_all(&run_dir).unwrap();
        std::fs::write(run_dir.join("output.f140.m4a.part"), b"x").unwrap();
        let user_file = dir.join("a.bin");
        std::fs::write(&user_file, b"finished file").unwrap();

        remove_download_artifacts(&save, "a.bin", id);
        assert!(!part.exists() && !state.exists() && !run_dir.exists());
        assert_eq!(std::fs::read(&user_file).unwrap(), b"finished file", "never the real file");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn intercept_auth_requires_token_origin_and_allowlist() {
        let ext = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
        let token = "live-hijack-token";
        let state = test_app_state(token, ext);
        let origin = format!("chrome-extension://{ext}");
        let mut headers = HeaderMap::new();
        headers.insert("x-falcon-token", token.parse().unwrap());
        assert!(check_api_token(&headers, &state, Some(&origin)).is_ok());
        assert_eq!(check_api_token(&headers, &state, None), Err(StatusCode::FORBIDDEN));
        let unknown = test_app_state(token, "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb");
        assert_eq!(check_api_token(&headers, &unknown, Some(&origin)), Err(StatusCode::FORBIDDEN));
        let mut bad = HeaderMap::new();
        bad.insert("x-falcon-token", "wrong".parse().unwrap());
        assert_eq!(check_api_token(&bad, &state, Some(&origin)), Err(StatusCode::UNAUTHORIZED));
        let legacy = test_app_state(LEGACY_DEFAULT_API_TOKEN, ext);
        assert_eq!(
            check_api_token(&headers, &legacy, Some(&origin)),
            Err(StatusCode::UNAUTHORIZED)
        );
    }
}
