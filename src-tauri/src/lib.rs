mod commands;
pub mod download;
pub mod extension_host;
mod lifecycle;
mod local_api;
pub mod log_buffer;
pub mod native_messaging;
pub mod settings;
pub mod storage;
pub mod util;

pub use lifecycle::run;

use chrono::Utc;
use download::engine::Aria2Engine;
use download::queue::QueueManager;
use native_messaging::PairProofStore;
use serde::Deserialize;
use settings::Settings;
use storage::models::{Download, DownloadCategory, DownloadStatus};
use storage::Database;
use storage::InsertDownloadResult;
use tauri::{AppHandle, Emitter, Manager};
use util::{
    app_data_dir, default_download_dir, full_file_path, inspect_download_dest, is_googlevideo_url,
    is_hls_url, is_junk_media_url, is_youtube_host, lock_or_recover, normalize_media_url,
    resolve_download_filename, resolve_download_target, resolve_save_dir, sanitize_filename,
    sanitize_header_value, unique_download_filename, validate_completed_file,
    validate_fetch_url_async, write_file_exclusive, ExistingDownloadDest, LEGACY_DEFAULT_API_TOKEN,
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

pub(crate) const MAX_PENDING_PAIR_REQUESTS: usize = 32;

pub(crate) fn extension_id_from_origin(origin: &str) -> Option<&str> {
    origin.strip_prefix("chrome-extension://").or_else(|| origin.strip_prefix("edge-extension://"))
}

pub(crate) fn is_valid_extension_id(id: &str) -> bool {
    id.len() == 32 && id.bytes().all(|byte| (b'a'..=b'p').contains(&byte))
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

/// Pause/resume (IPC + tray) must notify the UI — the 5s list poll and
/// sticky Paused ticks otherwise leave the row looking paused while it runs.
pub(crate) fn emit_download_status(app: &AppHandle, db: &Database, id: i64, status: &str) {
    let Ok(row) = db.get_download(id) else {
        return;
    };
    let _ = app.emit(
        "download-progress",
        serde_json::json!({
            "id": id,
            "downloaded_size": row.downloaded_size,
            "total_size": row.total_size.max(row.downloaded_size),
            "speed": 0.0,
            "status": status,
            "connections": 0,
            "error_message": row.error_message,
        }),
    );
}

/// `resume_if_current` zeros Failed bytes. Leftover `.part` / HLS / error-page dest
/// would resume corrupt data or block with "already exists".
pub(crate) fn clear_failed_retry_artifacts(save_path: &str, filename: &str, id: i64) {
    if let Some(part) = download::http::part_path_for(save_path, filename, id) {
        let _ = std::fs::remove_file(part);
    }
    if let Some(dir) = download::http::segment_dir_for(save_path, filename, id) {
        let _ = std::fs::remove_dir_all(dir);
    }
    let hls = download::hls::hls_session_dir(id);
    let _ = std::fs::remove_dir_all(&hls);
    if let Some(parent) = hls.parent() {
        let _ = std::fs::remove_dir_all(parent.join(format!("hls-{id}")));
    }
    let dest = full_file_path(save_path, filename);
    if inspect_download_dest(&dest) == ExistingDownloadDest::Occupied {
        let _ = std::fs::remove_file(&dest);
    }
}

pub struct AppState {
    pub db: Database,
    pub engine: Aria2Engine,
    pub queue: QueueManager,
    pub api_token: Arc<Mutex<String>>,
    pub settings: Arc<Mutex<Settings>>,
    pub rate_bucket: Arc<Mutex<VecDeque<Instant>>>,
    /// Extension IDs waiting for user approval (pair consent).
    pub pending_pair_ids: Arc<Mutex<VecDeque<String>>>,
    pub pair_proofs: Arc<PairProofStore>,
}

pub(crate) fn show_main_window(app: &AppHandle) {
    if let Some(window) = app.get_webview_window("main") {
        let _ = window.show();
        let _ = window.unminimize();
        let _ = window.set_focus();
    }
}

#[derive(Clone, Deserialize)]
pub(crate) struct ExternalDownloadPayload {
    url: String,
    filename: Option<String>,
    referrer: Option<String>,
    user_agent: Option<String>,
    cookies: Option<String>,
    cookie_url: Option<String>,
    title: Option<String>,
    /// yt-dlp `-f` selector (preferred over `#falconfmt=` fragment).
    format: Option<String>,
    save_path: Option<String>,
}

pub(crate) async fn enqueue_download(
    app: &AppHandle,
    payload: ExternalDownloadPayload,
) -> Result<i64, String> {
    if payload.url.trim().to_lowercase().starts_with("magnet:") {
        return Err("ERR_UNSUPPORTED_MAGNET".into());
    }
    if validate_fetch_url_async(&payload.url).await.is_err() {
        return Err("invalid url".into());
    }
    if is_junk_media_url(&payload.url) {
        return Err("not a real media url".into());
    }

    const DEFAULT_YTDLP_FORMAT: &str =
        "bestvideo[height<=1080]+bestaudio/best[height<=1080]/bv*+ba/b";
    let watch_url = util::youtube_page_url_for_download(&payload.url, payload.referrer.as_deref());
    let base = watch_url.clone().unwrap_or_else(|| normalize_media_url(&payload.url));
    let format = payload.format.as_deref().or(if watch_url.is_some() {
        Some(DEFAULT_YTDLP_FORMAT)
    } else {
        None
    });
    let url = util::attach_falcon_format(&base, format);
    let force_hls = is_hls_url(&url);
    let filename = resolve_download_filename(
        &url,
        payload.filename.as_deref(),
        payload.title.as_deref(),
        force_hls,
    );
    let category = DownloadCategory::from_filename(&filename);

    // ponytail: apply a per-site profile if one matches. Profile fields only fill
    // GAPS — an explicit value from the payload (the user/extension's choice)
    // always wins. save_subdir overrides the category folder when set.
    let settings = {
        let state = app.state::<AppState>();
        let cached = lock_or_recover(&state.settings).clone();
        cached
    };
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

    let state = app.state::<AppState>();

    let mut dl = Download {
        id: None,
        url: url.clone(),
        filename: filename.clone(),
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
        referrer: resolve_media_referer(
            &url,
            payload.referrer.as_deref().or(prof_referrer.as_deref()),
            payload.referrer.as_deref().or(prof_referrer.as_deref()),
        ),
        user_agent: payload.user_agent.clone().or(prof_ua).map(|s| sanitize_header_value(&s)),
        cookies: payload
            .cookies
            .filter(|_| cookie_url_matches_download(&url, payload.cookie_url.as_deref()))
            .or(prof_cookies)
            .map(|s| sanitize_header_value(&s)),
        aria2_gid: None,
        archived: false,
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
    let settings = {
        let state = app.state::<AppState>();
        let cached = lock_or_recover(&state.settings).clone();
        cached
    };
    let save_path = resolve_download_save_path_with(&settings, None, &category)?;
    let size = bytes.len() as u64;
    let now = Utc::now().to_rfc3339();
    let state = app.state::<AppState>();
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
        aria2_gid: None,
        archived: false,
    };
    let insert_result = state.db.insert_download_deduped(&mut dl).map_err(|e| e.to_string())?;
    let id = insert_result.id();
    if matches!(insert_result, InsertDownloadResult::Existing(_)) {
        return Ok(id);
    }
    let dest = resolve_download_target(&save_path, &dl.filename)?;
    if let Err(e) = write_file_exclusive(&dest, &bytes) {
        let _ = state.db.delete_download(id);
        return Err(format!("Could not save upload: {e}"));
    }
    dl.id = Some(id);
    let _ = app.emit("download-added", &dl);
    let _ = app.emit("download-completed", &dl);
    Ok(id)
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
    requested: Option<&str>,
    category: &DownloadCategory,
) -> Result<String, String> {
    resolve_download_save_path_with(&Settings::load(&app_data_dir()), requested, category)
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
        .or_else(|| settings.path_for_category(category.as_str()))
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

fn resolve_media_referer(
    url: &str,
    page_url: Option<&str>,
    referer: Option<&str>,
) -> Option<String> {
    let page = page_url.filter(|s| !s.is_empty());
    let r = referer.filter(|s| !s.is_empty()).or(page);
    let resolved = if is_googlevideo_url(url) || util::is_youtube_watch_url(url) {
        r.or(Some("https://www.youtube.com/"))
    } else {
        r
    };
    resolved.map(sanitize_header_value)
}

pub(crate) fn finalize_completed_download(dl: &mut Download) -> Result<(), String> {
    let file_path = full_file_path(&dl.save_path, &dl.filename);
    let size = validate_completed_file(&file_path)?;
    dl.downloaded_size = size;
    dl.total_size = size.max(dl.total_size);
    dl.status = DownloadStatus::Completed;
    dl.completed_at = Some(Utc::now().to_rfc3339());
    dl.error_message = None;
    dl.speed = 0.0;
    Ok(())
}

pub(crate) fn fail_invalid_download(dl: &mut Download, reason: String) {
    // Never delete dest — it may be a pre-existing user file (F-31 class).
    dl.status = DownloadStatus::Failed;
    dl.error_message = Some(reason);
    dl.speed = 0.0;
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
    fn fail_invalid_download_does_not_delete_existing_dest() {
        let dir = std::env::temp_dir().join(format!("falcon-fail-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let dest = dir.join("keep.bin");
        std::fs::write(&dest, b"user-bytes").unwrap();
        let mut dl = Download {
            id: Some(1),
            url: "https://example.com/a".into(),
            filename: "keep.bin".into(),
            save_path: dir.to_string_lossy().into(),
            total_size: 10,
            downloaded_size: 10,
            status: DownloadStatus::Downloading,
            category: DownloadCategory::Other,
            speed: 1.0,
            segments: 1,
            priority: 1,
            created_at: "t".into(),
            completed_at: None,
            error_message: None,
            referrer: None,
            user_agent: None,
            cookies: None,
            aria2_gid: None,
            archived: false,
        };
        fail_invalid_download(&mut dl, "invalid".into());
        assert_eq!(std::fs::read(&dest).unwrap(), b"user-bytes");
        assert_eq!(dl.status, DownloadStatus::Failed);
        let _ = std::fs::remove_file(&dest);
        let _ = std::fs::remove_dir(&dir);
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
    fn failed_retry_clears_occupied_part_not_complete() {
        let dir = std::env::temp_dir().join(format!("falcon-retry-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let save = dir.to_string_lossy().to_string();
        let retry_id = 1_000_000_000_000i64 + i64::from(std::process::id());
        let occupied = dir.join("bad.bin");
        std::fs::write(&occupied, b"<html>err</html>").unwrap();
        let complete = dir.join("keep.bin");
        std::fs::write(&complete, vec![0u8; 1024]).unwrap();
        let part = crate::download::http::part_path_for(&save, "bad.bin", retry_id).unwrap();
        std::fs::write(&part, b"partial").unwrap();
        let segs = crate::download::http::segment_dir_for(&save, "bad.bin", retry_id).unwrap();
        std::fs::create_dir_all(&segs).unwrap();
        std::fs::write(segs.join("x"), b"s").unwrap();

        clear_failed_retry_artifacts(&save, "bad.bin", retry_id);
        assert!(!occupied.exists());
        assert!(!part.exists());
        assert!(!segs.exists());

        clear_failed_retry_artifacts(&save, "keep.bin", retry_id + 1);
        assert_eq!(std::fs::read(&complete).unwrap().len(), 1024);
        let _ = std::fs::remove_dir_all(&dir);
    }

    fn hijack_api_state(token: &str, ext_id: &str) -> AppState {
        let settings =
            Settings { allowed_extension_ids: vec![ext_id.to_string()], ..Default::default() };
        AppState {
            db: Database::in_memory().unwrap(),
            engine: Aria2Engine::new(),
            queue: QueueManager::new(),
            api_token: Arc::new(Mutex::new(token.to_string())),
            settings: Arc::new(Mutex::new(settings)),
            rate_bucket: Arc::new(Mutex::new(VecDeque::new())),
            pending_pair_ids: Arc::new(Mutex::new(VecDeque::new())),
            pair_proofs: Arc::new(PairProofStore::default()),
        }
    }

    #[test]
    fn intercept_auth_requires_token_origin_and_allowlist() {
        let ext = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
        let token = "live-hijack-token";
        let state = hijack_api_state(token, ext);
        let origin = format!("chrome-extension://{ext}");
        let mut headers = HeaderMap::new();
        headers.insert("x-falcon-token", token.parse().unwrap());
        assert!(check_api_token(&headers, &state, Some(&origin)).is_ok());
        assert_eq!(check_api_token(&headers, &state, None), Err(StatusCode::FORBIDDEN));
        let unknown = hijack_api_state(token, "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb");
        assert_eq!(check_api_token(&headers, &unknown, Some(&origin)), Err(StatusCode::FORBIDDEN));
        let mut bad = HeaderMap::new();
        bad.insert("x-falcon-token", "wrong".parse().unwrap());
        assert_eq!(check_api_token(&bad, &state, Some(&origin)), Err(StatusCode::UNAUTHORIZED));
        let legacy = hijack_api_state(LEGACY_DEFAULT_API_TOKEN, ext);
        assert_eq!(
            check_api_token(&headers, &legacy, Some(&origin)),
            Err(StatusCode::UNAUTHORIZED)
        );
    }
}
