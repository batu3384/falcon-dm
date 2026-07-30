pub mod download;
pub mod settings;
pub mod storage;

use chrono::Utc;
use download::engine::Aria2Engine;
use serde::Serialize;
use storage::models::{Download, DownloadCategory, DownloadFilter, DownloadStatus};
use storage::Database;
use tauri::{
    menu::{Menu, MenuItem},
    tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent},
    Emitter, Manager, State,
};
use tauri_plugin_notification::NotificationExt;

use download::queue::{QueueManager, ScheduleOptions};

use axum::{extract::State as AxumState, routing::post, Json, Router};
use serde::Deserialize;
use settings::Settings;
use tauri::AppHandle;

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
}

#[tauri::command]
fn greet(name: &str) -> String {
    format!("Falcon DM karşılıyor: {}", name)
}

#[tauri::command]
async fn add_download(
    url: String,
    filename: String,
    save_path: String,
    state: State<'_, AppState>,
) -> Result<Download, String> {
    let mut dl = Download {
        id: None,
        url: url.clone(),
        filename: filename.clone(),
        save_path: save_path.clone(),
        total_size: 104857600, // Dummy total size to allow mocking progress smoothly
        downloaded_size: 0,
        status: DownloadStatus::Queued, // Start as queued
        category: DownloadCategory::from_filename(&filename),
        speed: 0.0,
        segments: 16,
        priority: 1,
        created_at: Utc::now().to_rfc3339(),
        completed_at: None,
        error_message: None,
        referrer: None,
        user_agent: None,
        cookies: None,
        aria2_gid: None,
    };

    let id = state.db.insert_download(&dl).map_err(|e| e.to_string())?;
    dl.id = Some(id);

    Ok(dl)
}

#[tauri::command]
async fn pause_download(id: i64, state: State<'_, AppState>) -> Result<(), String> {
    let mut dl = state.db.get_download(id).map_err(|e| e.to_string())?;
    if let Some(gid) = &dl.aria2_gid {
        let _ = state.engine.pause(gid).await;
    }
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

    // Instead of forcing the engine to resume (which bypasses queue limits),
    // we set it to Queued. The QueueManager will start it when there is capacity.
    dl.status = DownloadStatus::Queued;

    // Optionally bump priority so it starts sooner
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
            .map_err(|e| format!("{:?}", e))
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
        start_time,
        stop_time,
        active,
    });
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
fn get_settings(app: AppHandle) -> Result<Settings, String> {
    let app_data_dir = app.path().app_data_dir().map_err(|e| e.to_string())?;
    Ok(Settings::load(&app_data_dir))
}

#[tauri::command]
async fn save_settings(
    settings: Settings,
    app: AppHandle,
    state: State<'_, AppState>,
) -> Result<(), String> {
    let app_data_dir = app.path().app_data_dir().map_err(|e| e.to_string())?;
    settings.save(&app_data_dir)?;

    // Apply settings dynamically
    state
        .queue
        .set_concurrent_downloads(settings.max_concurrent_downloads as usize);
    // Aria2 network limits might require restart, but we can try to update options if engine supports it.
    // For now we just update queue manager.

    Ok(())
}

#[derive(Deserialize)]
struct AddDownloadRequest {
    url: String,
    filename: String,
    referrer: Option<String>,
    user_agent: Option<String>,
    cookies: Option<String>,
}

async fn handle_api_add(
    AxumState(app): AxumState<AppHandle>,
    Json(payload): Json<AddDownloadRequest>,
) -> Json<serde_json::Value> {
    let state = app.state::<AppState>();

    let dl = Download {
        id: None,
        url: payload.url,
        filename: payload.filename.clone(),
        save_path: dirs::download_dir()
            .map(|p| p.to_string_lossy().to_string())
            .unwrap_or_else(|| "~/Downloads".to_string()),
        total_size: 104857600,
        downloaded_size: 0,
        status: DownloadStatus::Queued, // Handled by QueueManager tick
        category: DownloadCategory::from_filename(&payload.filename),
        speed: 0.0,
        segments: 16,
        priority: 1,
        created_at: Utc::now().to_rfc3339(),
        completed_at: None,
        error_message: None,
        referrer: payload.referrer,
        user_agent: payload.user_agent,
        cookies: payload.cookies,
        aria2_gid: None,
    };

    match state.db.insert_download(&dl) {
        Ok(id) => Json(serde_json::json!({ "success": true, "id": id })),
        Err(e) => Json(serde_json::json!({ "success": false, "error": e.to_string() })),
    }
}

#[derive(Deserialize, Clone, Serialize)]
pub struct InterceptRequest {
    pub url: String,
    pub page_url: Option<String>,
    pub media_type: Option<String>,
}

async fn handle_intercept(
    AxumState(app): AxumState<AppHandle>,
    Json(payload): Json<InterceptRequest>,
) -> Json<serde_json::Value> {
    // Emit event to frontend so it can open the Add URL modal with this link prefilled
    match app.emit("intercepted-media", payload) {
        Ok(_) => Json(serde_json::json!({ "success": true })),
        Err(e) => Json(serde_json::json!({ "success": false, "error": e.to_string() })),
    }
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    let app_data_dir = std::path::Path::new(":memory:");
    let db = Database::init(app_data_dir).expect("Failed to init DB");

    let engine = Aria2Engine::new();

    let app_state = AppState {
        db,
        engine,
        queue: QueueManager::new(),
    };

    let app = tauri::Builder::default()
        .plugin(tauri_plugin_shell::init())
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_notification::init())
        .plugin(tauri_plugin_dialog::init())
        .setup(|app| {
            let app_handle = app.handle().clone();

            let app_data_dir = app.path().app_data_dir().unwrap_or_else(|_| std::path::PathBuf::from("."));
            let settings = Settings::load(&app_data_dir);
            let state = app_handle.state::<AppState>();
            state.queue.set_concurrent_downloads(settings.max_concurrent_downloads as usize);

            // Start aria2c using Tauri sidecar API
            let _ = state.engine.start(&app_handle);

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
                    "quit" => {
                        app.exit(0);
                    }
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
                            if let Ok(downloads) = state.db.get_downloads(&DownloadFilter { status: Some(DownloadStatus::Downloading), ..Default::default() }) {
                                for mut dl in downloads {
                                    if let Some(gid) = &dl.aria2_gid {
                                        let _ = state.engine.pause(gid).await;
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
                            if let Ok(downloads) = state.db.get_downloads(&DownloadFilter { status: Some(DownloadStatus::Paused), ..Default::default() }) {
                                for mut dl in downloads {
                                    if let Some(gid) = &dl.aria2_gid {
                                        let _ = state.engine.resume(gid).await;
                                    }
                                    dl.status = DownloadStatus::Downloading;
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
                    } = event {
                        let app = tray.app_handle();
                        if let Some(window) = app.get_webview_window("main") {
                            let _ = window.show();
                            let _ = window.set_focus();
                        }
                    }
                })
                .build(app)?;

            // Spawn Axum HTTP server
            let axum_app_handle = app_handle.clone();
            tauri::async_runtime::spawn(async move {
                use tower_http::cors::{CorsLayer, AllowOrigin};
                use tauri::http::HeaderValue;

                let cors = CorsLayer::new()
                    .allow_origin(AllowOrigin::predicate(|origin: &HeaderValue, _request_parts| {
                        if let Ok(origin_str) = origin.to_str() {
                            origin_str.starts_with("chrome-extension://") ||
                            origin_str.starts_with("moz-extension://") ||
                            origin_str.starts_with("edge-extension://")
                        } else {
                            false
                        }
                    }))
                    .allow_methods(tower_http::cors::Any)
                    .allow_headers(tower_http::cors::Any)
                    .allow_private_network(true);

                let app = Router::new()
                    .route("/api/add", post(handle_api_add))
                    .route("/api/intercept", post(handle_intercept))
                    .layer(cors)
                    .with_state(axum_app_handle);

                match tokio::net::TcpListener::bind("127.0.0.1:14201").await {
                    Ok(listener) => {
                        println!("Axum HTTP server listening on {}", listener.local_addr().unwrap());
                        if let Err(e) = axum::serve(listener, app).await {
                            log::error!("Axum server error: {}", e);
                        }
                    }
                    Err(e) => {
                        log::error!("Failed to bind Axum server on port 14201: {}. Is another instance running?", e);
                    }
                }
            });
            tauri::async_runtime::spawn(async move {
                let mut interval = tokio::time::interval(std::time::Duration::from_millis(500));
                loop {
                    interval.tick().await;
                    let state = app_handle.state::<AppState>();
                    let _ = state.queue.tick(&state.db, &state.engine, app_handle.clone()).await;

                    if let Ok(downloads) = state.db.get_downloads(&DownloadFilter {
                        status: Some(DownloadStatus::Downloading),
                        ..Default::default()
                    }) {
                        for mut dl in downloads {
                            let mut speed = 0.0;
                            let mut status_str = "Downloading".to_string();
                            let mut connections = 8;

                            if let Some(gid) = &dl.aria2_gid {
                                if let Ok(status) = state.engine.get_status(gid).await {
                                    if let Some(completed_len_str) = status.get("completedLength").and_then(|v| v.as_str()) {
                                        dl.downloaded_size = completed_len_str.parse().unwrap_or(dl.downloaded_size);
                                    }
                                    if let Some(total_len_str) = status.get("totalLength").and_then(|v| v.as_str()) {
                                        dl.total_size = total_len_str.parse().unwrap_or(dl.total_size);
                                    }
                                    if let Some(speed_str) = status.get("downloadSpeed").and_then(|v| v.as_str()) {
                                        speed = speed_str.parse().unwrap_or(0.0);
                                    }
                                    if let Some(conn_str) = status.get("connections").and_then(|v| v.as_str()) {
                                        connections = conn_str.parse().unwrap_or(8);
                                    }
                                    if let Some(s) = status.get("status").and_then(|v| v.as_str()) {
                                        if s == "complete" {
                                            dl.status = DownloadStatus::Completed;
                                            status_str = "Completed".to_string();
                                        } else if s == "paused" {
                                            dl.status = DownloadStatus::Paused;
                                            status_str = "Paused".to_string();
                                        } else if s == "error" {
                                            dl.status = DownloadStatus::Failed;
                                            dl.error_message = status.get("errorMessage").and_then(|v| v.as_str()).map(|s| s.to_string());
                                            status_str = "Failed".to_string();
                                        }
                                    }
                                } else {
                                    dl.status = DownloadStatus::Failed;
                                    dl.error_message = Some("aria2 is unreachable or GID is invalid".to_string());
                                    status_str = "Failed".to_string();
                                    speed = 0.0;
                                }
                            } else {
                                // Missing GID but in downloading state? Transition to error.
                                dl.status = DownloadStatus::Failed;
                                dl.error_message = Some("Download missing aria2 GID".to_string());
                                status_str = "Failed".to_string();
                                speed = 0.0;
                            }
                            dl.speed = speed;

                            if dl.downloaded_size >= dl.total_size && dl.total_size > 0 {
                                dl.status = DownloadStatus::Completed;
                                status_str = "Completed".to_string();
                                speed = 0.0;
                            }

                            let _ = state.db.update_download_progress(dl.id.unwrap(), dl.downloaded_size, speed, &dl.status);

                            if status_str == "Completed" {
                                let _ = app_handle.notification().builder()
                                    .title("Download Complete")
                                    .body(&dl.filename)
                                    .show();
                                if let Some(window) = app_handle.get_webview_window("main") {
                                    let _ = window.request_user_attention(Some(tauri::UserAttentionType::Informational));
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
            change_priority,
            get_settings,
            save_settings
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
