pub mod download;
pub mod settings;
pub mod storage;
pub mod util;

use chrono::Utc;
use download::engine::Aria2Engine;
use download::queue::{QueueManager, ScheduleOptions};
use serde::{Deserialize, Serialize};
use settings::Settings;
use storage::models::{Download, DownloadCategory, DownloadFilter, DownloadStatus};
use storage::Database;
use tauri::{
    menu::{Menu, MenuItem},
    tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent},
    AppHandle, Emitter, Manager, State,
};
use tauri_plugin_notification::NotificationExt;
use util::{
    app_data_dir, default_download_dir, full_file_path, is_hls_url, is_junk_media_url,
    normalize_media_url, resolve_download_filename, resolve_save_dir, sanitize_filename,
    sanitize_header_value, validate_completed_file, validate_download_url, validate_open_path,
    LEGACY_DEFAULT_API_TOKEN,
};
use window_vibrancy::{apply_vibrancy, NSVisualEffectMaterial};

use axum::{
    body::Body,
    extract::State as AxumState,
    http::{HeaderMap, Request, StatusCode},
    middleware::{from_fn_with_state, Next},
    response::{IntoResponse, Response},
    routing::{get, post},
    Json, Router,
};
use std::collections::VecDeque;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

#[derive(Clone, Serialize)]
struct ProgressPayload {
    id: i64,
    downloaded_size: u64,
    total_size: u64,
    speed: f64,
    status: String,
    connections: u32,
}

pub struct AppState {
    pub db: Database,
    pub engine: Aria2Engine,
    pub queue: QueueManager,
    pub api_token: Arc<Mutex<String>>,
    pub rate_bucket: Arc<Mutex<VecDeque<Instant>>>,
    /// Extension ID waiting for user approval (pair consent).
    pub pending_pair_id: Arc<Mutex<Option<String>>>,
}

fn check_rate_limit(state: &AppState) -> Result<(), StatusCode> {
    let mut bucket = state.rate_bucket.lock().unwrap();
    let now = Instant::now();
    let window = Duration::from_secs(60);
    while bucket.front().is_some_and(|t| now.duration_since(*t) > window) {
        bucket.pop_front();
    }
    if bucket.len() >= 120 {
        return Err(StatusCode::TOO_MANY_REQUESTS);
    }
    bucket.push_back(now);
    Ok(())
}

async fn rate_limit_middleware(
    AxumState(app): AxumState<AppHandle>,
    req: Request<Body>,
    next: Next,
) -> Result<Response, StatusCode> {
    let state = app.state::<AppState>();
    check_rate_limit(&state)?;
    Ok(next.run(req).await)
}


fn show_main_window(app: &AppHandle) {
    if let Some(window) = app.get_webview_window("main") {
        let _ = window.show();
        let _ = window.unminimize();
        let _ = window.set_focus();
    }
}

#[derive(Clone, Deserialize)]
struct ExternalDownloadPayload {
    url: String,
    filename: Option<String>,
    referrer: Option<String>,
    user_agent: Option<String>,
    cookies: Option<String>,
    title: Option<String>,
    /// yt-dlp `-f` selector (preferred over `#falconfmt=` fragment).
    format: Option<String>,
}

fn enqueue_download(app: &AppHandle, payload: ExternalDownloadPayload) -> Result<i64, String> {
    if validate_download_url(&payload.url).is_err() {
        return Err("invalid url".into());
    }
    if is_junk_media_url(&payload.url) {
        return Err("not a real media url".into());
    }

    let url = util::attach_falcon_format(
        &normalize_media_url(&payload.url),
        payload.format.as_deref(),
    );
    let force_hls = is_hls_url(&url);
    let filename = resolve_download_filename(
        &url,
        payload.filename.as_deref(),
        payload.title.as_deref(),
        force_hls,
    );
    let category = DownloadCategory::from_filename(&filename);
    let save_path = resolve_download_save_path(None, &category)?;

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
            payload.referrer.as_deref(),
            payload.referrer.as_deref(),
        ),
        user_agent: payload
            .user_agent
            .as_deref()
            .map(sanitize_header_value),
        cookies: payload.cookies.as_deref().map(sanitize_header_value),
        aria2_gid: None,
    };

    let state = app.state::<AppState>();
    let id = state.db.insert_download(&dl).map_err(|e| e.to_string())?;
    dl.id = Some(id);
    let _ = app.emit("download-added", &dl);
    Ok(id)
}

fn handle_deep_link_url(app: &AppHandle, raw: &str) {
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

fn current_settings() -> Settings {
    Settings::load(&app_data_dir())
}

fn resolve_download_save_path(
    requested: Option<&str>,
    category: &DownloadCategory,
) -> Result<String, String> {
    let settings = current_settings();
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

    let preferred = settings
        .path_for_category(category.as_str())
        .or_else(|| {
            requested
                .map(|s| s.trim().to_string())
                .filter(|s| !s.is_empty())
        })
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

fn check_api_token(
    headers: &HeaderMap,
    state: &AppState,
    origin: Option<&str>,
) -> Result<(), StatusCode> {
    let expected = state.api_token.lock().unwrap().clone();
    if expected.trim().is_empty() || expected == LEGACY_DEFAULT_API_TOKEN {
        return Err(StatusCode::UNAUTHORIZED);
    }
    let provided = headers
        .get("x-falcon-token")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("");
    if provided != expected {
        return Err(StatusCode::UNAUTHORIZED);
    }

    // Origin required — blocks bare curl with stolen token (must forge extension Origin + allowlist).
    let origin = origin.filter(|s| !s.is_empty()).ok_or(StatusCode::FORBIDDEN)?;
    let ext_id = origin
        .strip_prefix("chrome-extension://")
        .or_else(|| origin.strip_prefix("moz-extension://"))
        .or_else(|| origin.strip_prefix("edge-extension://"))
        .ok_or(StatusCode::FORBIDDEN)?;
    if ext_id.is_empty() {
        return Err(StatusCode::FORBIDDEN);
    }
    let settings = Settings::load(&app_data_dir());
    if settings.allowed_extension_ids.is_empty()
        || !settings.allowed_extension_ids.iter().any(|x| x == ext_id)
    {
        return Err(StatusCode::FORBIDDEN);
    }
    Ok(())
}

fn resolve_media_referer(url: &str, page_url: Option<&str>, referer: Option<&str>) -> Option<String> {
    let page = page_url.filter(|s| !s.is_empty());
    let r = referer.filter(|s| !s.is_empty()).or(page);
    let resolved = if url.contains("googlevideo") || url.contains("youtube.com") {
        r.or(Some("https://www.youtube.com/"))
    } else {
        r
    };
    resolved.map(|s| sanitize_header_value(s))
}

fn finalize_completed_download(dl: &mut Download) -> Result<(), String> {
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

fn fail_invalid_download(dl: &mut Download, reason: String) {
    let file_path = full_file_path(&dl.save_path, &dl.filename);
    let _ = std::fs::remove_file(&file_path);
    dl.status = DownloadStatus::Failed;
    dl.error_message = Some(reason);
    dl.speed = 0.0;
}

#[tauri::command]
fn greet(name: &str) -> String {
    format!("Falcon DM karşılıyor: {}", name)
}

#[tauri::command]
async fn add_download(
    app: AppHandle,
    url: String,
    filename: String,
    save_path: String,
    referrer: Option<String>,
    user_agent: Option<String>,
    cookies: Option<String>,
    state: State<'_, AppState>,
) -> Result<Download, String> {
    validate_download_url(&url)?;
    let filename = sanitize_filename(&filename);
    let category = DownloadCategory::from_filename(&filename);
    let save_path = resolve_download_save_path(Some(&save_path), &category)?;

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
        referrer: referrer.map(|s| sanitize_header_value(&s)),
        user_agent: user_agent.map(|s| sanitize_header_value(&s)),
        cookies: cookies.map(|s| sanitize_header_value(&s)),
        aria2_gid: None,
    };

    let id = state.db.insert_download(&dl).map_err(|e| e.to_string())?;
    dl.id = Some(id);
    let _ = app.emit("download-added", &dl);
    Ok(dl)
}

#[tauri::command]
async fn pause_download(id: i64, state: State<'_, AppState>) -> Result<(), String> {
    let mut dl = state.db.get_download(id).map_err(|e| e.to_string())?;
    if let Some(gid) = &dl.aria2_gid {
        let _ = state.engine.pause(gid).await;
    }
    // Cancels HLS and yt-dlp tasks (shared watch map)
    state.queue.cancel_stream(id);
    dl.status = DownloadStatus::Paused;
    state
        .db
        .update_download(id, &dl)
        .map_err(|e| e.to_string())?;
    Ok(())
}

#[tauri::command]
async fn resume_download(id: i64, state: State<'_, AppState>) -> Result<(), String> {
    let mut dl = state.db.get_download(id).map_err(|e| e.to_string())?;
    if dl.status == DownloadStatus::Failed {
        dl.aria2_gid = None;
        dl.error_message = None;
    }
    dl.status = DownloadStatus::Queued;
    dl.priority += 1;
    state
        .db
        .update_download(id, &dl)
        .map_err(|e| e.to_string())?;
    Ok(())
}

#[tauri::command]
async fn remove_download(id: i64, state: State<'_, AppState>) -> Result<(), String> {
    let dl = state.db.get_download(id).map_err(|e| e.to_string())?;
    if let Some(gid) = &dl.aria2_gid {
        let _ = state.engine.remove(gid).await;
    }
    state.queue.cancel_stream(id);
    state.db.delete_download(id).map_err(|e| e.to_string())?;
    Ok(())
}

#[tauri::command]
async fn get_downloads(
    filter: DownloadFilter,
    state: State<'_, AppState>,
) -> Result<Vec<Download>, String> {
    state.db.get_downloads(&filter).map_err(|e| e.to_string())
}

#[tauri::command]
async fn get_download_status(
    id: i64,
    state: State<'_, AppState>,
) -> Result<serde_json::Value, String> {
    let dl = state.db.get_download(id).map_err(|e| e.to_string())?;
    if let Some(gid) = dl.aria2_gid {
        state
            .engine
            .get_status(&gid)
            .await
            .map_err(|e| e.to_string())
    } else {
        Err("No aria2 GID for download".to_string())
    }
}

#[tauri::command]
fn set_schedule(
    start_time: Option<String>,
    stop_time: Option<String>,
    active: bool,
    state: State<'_, AppState>,
) -> Result<(), String> {
    state.queue.set_schedule(ScheduleOptions {
        start_time: start_time.clone(),
        stop_time: stop_time.clone(),
        active,
    });
    let mut settings = Settings::load(&app_data_dir());
    settings.schedule_active = active;
    settings.schedule_start = start_time;
    settings.schedule_stop = stop_time;
    settings.save(&app_data_dir())?;
    Ok(())
}

#[tauri::command]
fn get_schedule(state: State<'_, AppState>) -> Result<ScheduleOptions, String> {
    Ok(state.queue.get_schedule())
}

#[tauri::command]
async fn open_folder(path: String) -> Result<(), String> {
    let safe = validate_open_path(&path)?;
    #[cfg(target_os = "macos")]
    {
        let _ = std::process::Command::new("open")
            .arg("-R")
            .arg(&safe)
            .spawn();
    }
    Ok(())
}

#[tauri::command]
async fn open_file(path: String) -> Result<(), String> {
    let safe = validate_open_path(&path)?;
    #[cfg(target_os = "macos")]
    {
        let _ = std::process::Command::new("open").arg(&safe).spawn();
    }
    Ok(())
}

#[tauri::command]
async fn change_priority(
    id: i64,
    increase: bool,
    state: State<'_, AppState>,
) -> Result<(), String> {
    let mut dl = state.db.get_download(id).map_err(|e| e.to_string())?;
    if increase {
        dl.priority += 1;
    } else if dl.priority > 0 {
        dl.priority -= 1;
    }
    state
        .db
        .update_download(id, &dl)
        .map_err(|e| e.to_string())?;
    Ok(())
}

#[tauri::command]
fn get_settings() -> Result<Settings, String> {
    Ok(Settings::load(&app_data_dir()))
}

#[tauri::command]
fn get_api_token(state: State<'_, AppState>) -> Result<String, String> {
    Ok(state.api_token.lock().unwrap().clone())
}

#[tauri::command]
fn reset_extension_pin(state: State<'_, AppState>) -> Result<(), String> {
    let mut settings = Settings::load(&app_data_dir());
    settings.allowed_extension_ids.clear();
    *state.pending_pair_id.lock().unwrap() = None;
    settings.save(&app_data_dir())
}

#[tauri::command]
fn get_pending_pair(state: State<'_, AppState>) -> Result<Option<String>, String> {
    Ok(state.pending_pair_id.lock().unwrap().clone())
}

#[tauri::command]
fn approve_extension_pair(
    extension_id: String,
    state: State<'_, AppState>,
) -> Result<(), String> {
    let id = extension_id.trim();
    if id.is_empty() {
        return Err("empty extension id".into());
    }
    let mut settings = Settings::load(&app_data_dir());
    if !settings.allowed_extension_ids.iter().any(|x| x == id) {
        settings.allowed_extension_ids.push(id.to_string());
    }
    settings.save(&app_data_dir())?;
    *state.pending_pair_id.lock().unwrap() = None;
    Ok(())
}

#[tauri::command]
async fn save_settings(
    settings: Settings,
    state: State<'_, AppState>,
) -> Result<(), String> {
    let dir = app_data_dir();
    settings.save(&dir)?;

    state
        .queue
        .set_concurrent_downloads(settings.max_concurrent_downloads as usize);
    state
        .queue
        .set_max_connections(settings.max_connections_per_server as usize);
    *state.api_token.lock().unwrap() = {
        let t = settings.api_token.trim();
        if t.is_empty() || t == LEGACY_DEFAULT_API_TOKEN {
            let fresh = uuid::Uuid::new_v4().to_string();
            let mut s = settings.clone();
            s.api_token = fresh.clone();
            let _ = s.save(&dir);
            fresh
        } else {
            settings.api_token.clone()
        }
    };

    if state.engine.is_running() {
        let _ = state.engine.apply_speed_limit(settings.speed_limit_kbps).await;
        let _ = state
            .engine
            .apply_proxy(settings.proxy.as_deref())
            .await;
    }

    Ok(())
}

#[derive(Deserialize)]
struct AddDownloadRequest {
    url: String,
    filename: String,
    referrer: Option<String>,
    user_agent: Option<String>,
    cookies: Option<String>,
    format: Option<String>,
}

async fn handle_api_add(
    AxumState(app): AxumState<AppHandle>,
    headers: HeaderMap,
    Json(payload): Json<AddDownloadRequest>,
) -> Result<Json<serde_json::Value>, StatusCode> {
    let state = app.state::<AppState>();
    check_api_token(
        &headers,
        &state,
        headers.get("origin").and_then(|v| v.to_str().ok()),
    )?;

    if validate_download_url(&payload.url).is_err() {
        return Ok(Json(serde_json::json!({ "success": false, "error": "invalid url" })));
    }

    let ext = ExternalDownloadPayload {
        url: payload.url,
        filename: Some(payload.filename),
        referrer: payload.referrer,
        user_agent: payload.user_agent,
        cookies: payload.cookies,
        title: None,
        format: payload.format,
    };

    match enqueue_download(&app, ext) {
        Ok(id) => Ok(Json(serde_json::json!({ "success": true, "id": id }))),
        Err(e) => Ok(Json(serde_json::json!({ "success": false, "error": e }))),
    }
}

#[derive(Deserialize, Clone, Serialize)]
pub struct InterceptRequest {
    pub url: String,
    pub page_url: Option<String>,
    pub media_type: Option<String>,
    pub title: Option<String>,
    pub cookies: Option<String>,
    pub user_agent: Option<String>,
    pub referer: Option<String>,
    pub filename: Option<String>,
    pub format: Option<String>,
}

async fn handle_intercept(
    AxumState(app): AxumState<AppHandle>,
    headers: HeaderMap,
    Json(payload): Json<InterceptRequest>,
) -> Result<Json<serde_json::Value>, StatusCode> {
    let state = app.state::<AppState>();
    check_api_token(
        &headers,
        &state,
        headers.get("origin").and_then(|v| v.to_str().ok()),
    )?;

    if validate_download_url(&payload.url).is_err() {
        return Ok(Json(serde_json::json!({ "success": false, "error": "invalid url" })));
    }

    let ext = ExternalDownloadPayload {
        url: payload.url,
        filename: payload.filename,
        referrer: payload
            .referer
            .or(payload.page_url.clone()),
        user_agent: payload.user_agent,
        cookies: payload.cookies,
        title: payload.title,
        format: payload.format,
    };

    match enqueue_download(&app, ext) {
        Ok(id) => Ok(Json(serde_json::json!({ "success": true, "id": id }))),
        Err(e) => Ok(Json(serde_json::json!({ "success": false, "error": e }))),
    }
}

async fn handle_health() -> Json<serde_json::Value> {
    Json(serde_json::json!({ "ok": true, "service": "falcon-dm" }))
}

/// Pair browser extension ↔ desktop. Requires prior user approval (no first-wins auto-pin).
async fn handle_pair(
    AxumState(app): AxumState<AppHandle>,
    headers: HeaderMap,
) -> Response {
    let origin = headers
        .get("origin")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("");
    let ext_id = origin
        .strip_prefix("chrome-extension://")
        .or_else(|| origin.strip_prefix("moz-extension://"))
        .or_else(|| origin.strip_prefix("edge-extension://"));
    let Some(id) = ext_id.filter(|s| !s.is_empty()) else {
        return StatusCode::FORBIDDEN.into_response();
    };

    let state = app.state::<AppState>();
    let token = state.api_token.lock().unwrap().clone();
    if token.trim().is_empty() || token == LEGACY_DEFAULT_API_TOKEN {
        return StatusCode::SERVICE_UNAVAILABLE.into_response();
    }

    let settings = Settings::load(&app_data_dir());
    if settings.allowed_extension_ids.iter().any(|x| x == id) {
        return (
            StatusCode::OK,
            Json(serde_json::json!({
                "ok": true,
                "token": token,
                "extension_id": id,
            })),
        )
            .into_response();
    }

    // Pending consent — UI must approve
    *state.pending_pair_id.lock().unwrap() = Some(id.to_string());
    let _ = app.emit(
        "pair-request",
        serde_json::json!({ "extension_id": id }),
    );

    (
        StatusCode::ACCEPTED,
        Json(serde_json::json!({
            "ok": false,
            "pending": true,
            "extension_id": id,
        })),
    )
        .into_response()
}

async fn handle_ping(
    AxumState(app): AxumState<AppHandle>,
    headers: HeaderMap,
) -> Result<Json<serde_json::Value>, StatusCode> {
    let state = app.state::<AppState>();
    check_api_token(
        &headers,
        &state,
        headers.get("origin").and_then(|v| v.to_str().ok()),
    )?;
    Ok(Json(serde_json::json!({ "ok": true })))
}

fn restore_orphan_downloads(db: &Database) {
    if let Ok(active) = db.get_downloads(&DownloadFilter {
        status: Some(DownloadStatus::Downloading),
        ..Default::default()
    }) {
        for mut dl in active {
            // Crash recovery: clear stale engine state for all orphans
            dl.aria2_gid = None;
            dl.status = DownloadStatus::Queued;
            dl.speed = 0.0;
            if let Some(id) = dl.id {
                let _ = db.update_download(id, &dl);
            }
        }
    }
    if let Ok(merging) = db.get_downloads(&DownloadFilter {
        status: Some(DownloadStatus::Merging),
        ..Default::default()
    }) {
        for mut dl in merging {
            dl.aria2_gid = None;
            dl.status = DownloadStatus::Queued;
            if let Some(id) = dl.id {
                let _ = db.update_download(id, &dl);
            }
        }
    }
    // Cookie TTL: wipe session cookies from finished jobs (at-rest hygiene)
    if let Ok(done) = db.get_downloads(&DownloadFilter {
        status: Some(DownloadStatus::Completed),
        ..Default::default()
    }) {
        for mut dl in done {
            if dl.cookies.is_some() {
                dl.cookies = None;
                if let Some(id) = dl.id {
                    let _ = db.update_download(id, &dl);
                }
            }
        }
    }
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    let data_dir = app_data_dir();
    let _ = std::fs::create_dir_all(&data_dir);
    let db = Database::init(&data_dir).expect("Failed to init DB");
    restore_orphan_downloads(&db);

    let settings = Settings::load(&data_dir);
    let engine = Aria2Engine::new();
    let queue = QueueManager::new();
    queue.set_concurrent_downloads(settings.max_concurrent_downloads as usize);
    queue.set_max_connections(settings.max_connections_per_server as usize);
    queue.set_schedule(ScheduleOptions {
        start_time: settings.schedule_start.clone(),
        stop_time: settings.schedule_stop.clone(),
        active: settings.schedule_active,
    });

    let app_state = AppState {
        db,
        engine,
        queue,
        api_token: Arc::new(Mutex::new(settings.api_token.clone())),
        rate_bucket: Arc::new(Mutex::new(VecDeque::new())),
        pending_pair_id: Arc::new(Mutex::new(None)),
    };

    let app = tauri::Builder::default()
        .plugin(tauri_plugin_deep_link::init())
        .plugin(tauri_plugin_shell::init())
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_notification::init())
        .plugin(tauri_plugin_dialog::init())
        .setup(|app| {
            let app_handle = app.handle().clone();
            let state = app_handle.state::<AppState>();
            let data_dir = app_data_dir();

            #[cfg(desktop)]
            {
                use tauri_plugin_deep_link::DeepLinkExt;
                let dl_handle = app_handle.clone();
                app.deep_link().on_open_url(move |event| {
                    for url in event.urls() {
                        handle_deep_link_url(&dl_handle, url.as_str());
                    }
                });
            }

            #[cfg(target_os = "macos")]
            if let Some(window) = app.get_webview_window("main") {
                let _ = apply_vibrancy(
                    &window,
                    NSVisualEffectMaterial::UnderWindowBackground,
                    None,
                    None,
                );
            }

            match state.engine.start(&app_handle, &data_dir) {
                Ok(()) => {
                    let handle = app_handle.clone();
                    tauri::async_runtime::spawn(async move {
                        let st = handle.state::<AppState>();
                        match st.engine.wait_ready(40).await {
                            Ok(()) => {
                                let s = Settings::load(&app_data_dir());
                                let _ = st.engine.apply_speed_limit(s.speed_limit_kbps).await;
                                let _ = st.engine.apply_proxy(s.proxy.as_deref()).await;
                            }
                            Err(e) => {
                                log::error!("aria2 not ready after start: {}", e);
                                // One reclaim+restart attempt
                                let dir = app_data_dir();
                                if st.engine.start(&handle, &dir).is_ok() {
                                    if let Err(e2) = st.engine.wait_ready(40).await {
                                        log::error!("aria2 restart also failed: {}", e2);
                                        st.engine.mark_not_running();
                                    } else {
                                        let s = Settings::load(&app_data_dir());
                                        let _ = st.engine.apply_speed_limit(s.speed_limit_kbps).await;
                                        let _ = st.engine.apply_proxy(s.proxy.as_deref()).await;
                                    }
                                } else {
                                    st.engine.mark_not_running();
                                }
                            }
                        }
                    });
                }
                Err(e) => {
                    log::error!("{}", e);
                }
            }

            let quit_i = MenuItem::with_id(app, "quit", "Quit", true, None::<&str>)?;
            let show_i = MenuItem::with_id(app, "show", "Show Falcon DM", true, None::<&str>)?;
            let pause_i = MenuItem::with_id(app, "pause", "Pause All", true, None::<&str>)?;
            let resume_i = MenuItem::with_id(app, "resume", "Resume All", true, None::<&str>)?;
            let menu = Menu::with_items(app, &[&show_i, &pause_i, &resume_i, &quit_i])?;

            let _tray = TrayIconBuilder::new()
                .icon(app.default_window_icon().unwrap().clone())
                .menu(&menu)
                .show_menu_on_left_click(false)
                .on_menu_event(|app, event| match event.id.as_ref() {
                    "quit" => app.exit(0),
                    "show" => {
                        if let Some(window) = app.get_webview_window("main") {
                            let _ = window.show();
                            let _ = window.set_focus();
                        }
                    }
                    "pause" => {
                        let app_h = app.clone();
                        tauri::async_runtime::spawn(async move {
                            let state = app_h.state::<AppState>();
                            if let Ok(downloads) = state.db.get_downloads(&DownloadFilter {
                                status: Some(DownloadStatus::Downloading),
                                ..Default::default()
                            }) {
                                for mut dl in downloads {
                                    if let Some(gid) = &dl.aria2_gid {
                                        let _ = state.engine.pause(gid).await;
                                    }
                                    if is_hls_url(&dl.url) {
                                        if let Some(id) = dl.id {
                                            state.queue.cancel_stream(id);
                                        }
                                    }
                                    dl.status = DownloadStatus::Paused;
                                    let _ = state.db.update_download(dl.id.unwrap(), &dl);
                                }
                            }
                        });
                    }
                    "resume" => {
                        let app_h = app.clone();
                        tauri::async_runtime::spawn(async move {
                            let state = app_h.state::<AppState>();
                            if let Ok(downloads) = state.db.get_downloads(&DownloadFilter {
                                status: Some(DownloadStatus::Paused),
                                ..Default::default()
                            }) {
                                for mut dl in downloads {
                                    // Respect queue concurrency — mark Queued, let tick start
                                    dl.status = DownloadStatus::Queued;
                                    let _ = state.db.update_download(dl.id.unwrap(), &dl);
                                }
                            }
                        });
                    }
                    _ => {}
                })
                .on_tray_icon_event(|tray, event| {
                    if let TrayIconEvent::Click {
                        button: MouseButton::Left,
                        button_state: MouseButtonState::Up,
                        ..
                    } = event
                    {
                        let app = tray.app_handle();
                        if let Some(window) = app.get_webview_window("main") {
                            let _ = window.show();
                            let _ = window.set_focus();
                        }
                    }
                })
                .build(app)?;

            let axum_app_handle = app_handle.clone();
            tauri::async_runtime::spawn(async move {
                use tower_http::cors::{AllowOrigin, CorsLayer};
                use tauri::http::HeaderValue;

                let cors = CorsLayer::new()
                    .allow_origin(AllowOrigin::predicate(|origin: &HeaderValue, _| {
                        if let Ok(origin_str) = origin.to_str() {
                            origin_str.starts_with("chrome-extension://")
                                || origin_str.starts_with("moz-extension://")
                                || origin_str.starts_with("edge-extension://")
                        } else {
                            false
                        }
                    }))
                    .allow_methods(tower_http::cors::Any)
                    .allow_headers(tower_http::cors::Any)
                    .allow_private_network(true);

                let app = Router::new()
                    .route("/api/health", get(handle_health))
                    .route("/api/pair", post(handle_pair))
                    .route("/api/ping", post(handle_ping))
                    .route("/api/add", post(handle_api_add))
                    .route("/api/intercept", post(handle_intercept))
                    .layer(from_fn_with_state(
                        axum_app_handle.clone(),
                        rate_limit_middleware,
                    ))
                    .layer(cors)
                    .with_state(axum_app_handle);

                match tokio::net::TcpListener::bind("127.0.0.1:14201").await {
                    Ok(listener) => {
                        log::info!(
                            "Axum HTTP server listening on {}",
                            listener.local_addr().unwrap()
                        );
                        if let Err(e) = axum::serve(listener, app).await {
                            log::error!("Axum server error: {}", e);
                        }
                    }
                    Err(e) => {
                        log::error!(
                            "Failed to bind Axum server on port 14201: {}. Is another instance running?",
                            e
                        );
                    }
                }
            });

            tauri::async_runtime::spawn(async move {
                let mut interval = tokio::time::interval(std::time::Duration::from_millis(500));
                loop {
                    interval.tick().await;
                    let state = app_handle.state::<AppState>();
                    let _ = state
                        .queue
                        .tick(&state.db, &state.engine, app_handle.clone())
                        .await;

                    if let Ok(downloads) = state.db.get_downloads(&DownloadFilter {
                        status: Some(DownloadStatus::Downloading),
                        ..Default::default()
                    }) {
                        for mut dl in downloads {
                            let mut speed = 0.0;
                            let mut status_str = "Downloading".to_string();
                            let mut connections = 8u32;

                            if let Some(gid) = &dl.aria2_gid {
                                match state.engine.get_status(gid).await {
                                    Ok(status) => {
                                        if let Some(v) = status
                                            .get("completedLength")
                                            .and_then(|v| v.as_str())
                                        {
                                            dl.downloaded_size =
                                                v.parse().unwrap_or(dl.downloaded_size);
                                        }
                                        if let Some(v) =
                                            status.get("totalLength").and_then(|v| v.as_str())
                                        {
                                            dl.total_size = v.parse().unwrap_or(dl.total_size);
                                        }
                                        if let Some(v) =
                                            status.get("downloadSpeed").and_then(|v| v.as_str())
                                        {
                                            speed = v.parse().unwrap_or(0.0);
                                        }
                                        if let Some(v) =
                                            status.get("connections").and_then(|v| v.as_str())
                                        {
                                            connections = v.parse().unwrap_or(8);
                                        }
                                        if let Some(s) =
                                            status.get("status").and_then(|v| v.as_str())
                                        {
                                            match s {
                                                "complete" => {
                                                    if let Err(e) = finalize_completed_download(&mut dl) {
                                                        fail_invalid_download(&mut dl, e);
                                                        status_str = "Failed".into();
                                                    } else {
                                                        status_str = "Completed".into();
                                                    }
                                                }
                                                "paused" => {
                                                    dl.status = DownloadStatus::Paused;
                                                    status_str = "Paused".into();
                                                }
                                                "error" => {
                                                    dl.status = DownloadStatus::Failed;
                                                    dl.error_message = status
                                                        .get("errorMessage")
                                                        .and_then(|v| v.as_str())
                                                        .map(|s| s.to_string());
                                                    status_str = "Failed".into();
                                                }
                                                _ => {}
                                            }
                                        }
                                    }
                                    Err(e) => {
                                        let msg = e.to_string().to_lowercase();
                                        // Transient: aria2 restarting / RPC blip — skip this tick
                                        if msg.contains("not running")
                                            || msg.contains("network")
                                            || msg.contains("timed out")
                                            || msg.contains("connection")
                                        {
                                            continue;
                                        }
                                        dl.status = DownloadStatus::Failed;
                                        dl.error_message = Some(e.to_string());
                                        status_str = "Failed".into();
                                        speed = 0.0;
                                    }
                                }
                            } else if is_hls_url(&dl.url) {
                                // HLS progress is emitted by the HLS task itself
                                continue;
                            } else {
                                // Non-HLS downloading without GID: wait a tick for queue to assign
                                continue;
                            }

                            dl.speed = speed;

                            if dl.downloaded_size >= dl.total_size
                                && dl.total_size > 0
                                && status_str == "Downloading"
                            {
                                if let Err(e) = finalize_completed_download(&mut dl) {
                                    fail_invalid_download(&mut dl, e);
                                    status_str = "Failed".into();
                                } else {
                                    status_str = "Completed".into();
                                }
                                speed = 0.0;
                            }

                            let _ = state.db.update_download(dl.id.unwrap(), &dl);

                            if status_str == "Completed" {
                                // Drop cookies after completion (session hygiene)
                                dl.cookies = None;
                                let _ = state.db.update_download(dl.id.unwrap(), &dl);

                                let _ = app_handle
                                    .notification()
                                    .builder()
                                    .title("Download Complete")
                                    .body(&dl.filename)
                                    .show();
                                if let Some(window) = app_handle.get_webview_window("main") {
                                    let _ = window.request_user_attention(Some(
                                        tauri::UserAttentionType::Informational,
                                    ));
                                }
                            }

                            let payload = ProgressPayload {
                                id: dl.id.unwrap(),
                                downloaded_size: dl.downloaded_size,
                                total_size: dl.total_size,
                                speed,
                                status: status_str,
                                connections,
                            };
                            let _ = app_handle.emit("download-progress", payload);
                        }
                    }
                }
            });
            Ok(())
        })
        .manage(app_state)
        .invoke_handler(tauri::generate_handler![
            greet,
            add_download,
            pause_download,
            resume_download,
            remove_download,
            get_downloads,
            get_download_status,
            set_schedule,
            get_schedule,
            change_priority,
            get_settings,
            save_settings,
            get_api_token,
            reset_extension_pin,
            get_pending_pair,
            approve_extension_pair,
            open_folder,
            open_file
        ])
        .build(tauri::generate_context!())
        .expect("error while building tauri application");

    app.run(|app_handle, event| {
        if let tauri::RunEvent::ExitRequested { .. } = event {
            let state = app_handle.state::<AppState>();
            let _ = state.engine.stop();
            log::info!("Shutting down Falcon DM, stopped aria2 engine.");
        }
    });
}
