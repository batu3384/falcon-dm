mod commands;
pub mod download;
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
use tauri::{AppHandle, Emitter, Manager};
use util::{
    app_data_dir, default_download_dir, full_file_path, is_hls_url, is_junk_media_url,
    lock_or_recover, normalize_media_url, resolve_download_filename, resolve_save_dir,
    sanitize_header_value, validate_completed_file, validate_fetch_url_async,
    LEGACY_DEFAULT_API_TOKEN,
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
    origin
        .strip_prefix("chrome-extension://")
        .or_else(|| origin.strip_prefix("moz-extension://"))
        .or_else(|| origin.strip_prefix("edge-extension://"))
}

pub(crate) fn is_valid_extension_id(id: &str) -> bool {
    id.len() == 32 && id.bytes().all(|byte| (b'a'..=b'p').contains(&byte))
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
    matches!(download.scheme(), "https")
        && download.scheme() == cookie_source.scheme()
        && download.host_str().map(str::to_ascii_lowercase)
            == cookie_source.host_str().map(str::to_ascii_lowercase)
        && download.port_or_known_default() == cookie_source.port_or_known_default()
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
    if validate_fetch_url_async(&payload.url).await.is_err() {
        return Err("invalid url".into());
    }
    if is_junk_media_url(&payload.url) {
        return Err("not a real media url".into());
    }

    let url =
        util::attach_falcon_format(&normalize_media_url(&payload.url), payload.format.as_deref());
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

    let prof_referrer = profile.and_then(|p| p.referrer.clone());
    let prof_ua = profile.and_then(|p| p.user_agent.clone());
    let prof_cookies = profile.and_then(|p| p.cookies.clone());

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
        segments: 16,
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

    let state = app.state::<AppState>();
    let id = state.db.insert_download(&dl).map_err(|e| e.to_string())?;
    dl.id = Some(id);
    let _ = app.emit("download-added", &dl);
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
        if std::fs::create_dir_all(&expanded).is_ok() {
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
    let resolved = if url.contains("googlevideo") || url.contains("youtube.com") {
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
    let file_path = full_file_path(&dl.save_path, &dl.filename);
    let _ = std::fs::remove_file(&file_path);
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
        assert!(!is_valid_extension_id("short"));
        assert!(!is_valid_extension_id("zzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzz"));
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
    }
}
