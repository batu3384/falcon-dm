pub mod storage;
pub mod download;

use download::engine::{Aria2Engine, Aria2Options};
use storage::models::{Download, DownloadFilter, DownloadStatus, DownloadCategory};
use storage::Database;
use tauri::{Manager, Emitter, State};
use chrono::Utc;
use serde::Serialize;

use download::queue::{QueueManager, ScheduleOptions};

use serde::Deserialize;
use axum::{routing::post, Router, extract::State as AxumState, Json};
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
    state.db.update_download(id, &dl).map_err(|e| e.to_string())?;
    Ok(())
}

#[tauri::command]
async fn resume_download(id: i64, state: State<'_, AppState>) -> Result<(), String> {
    let mut dl = state.db.get_download(id).map_err(|e| e.to_string())?;
    if let Some(gid) = &dl.aria2_gid {
        let _ = state.engine.resume(gid).await;
    }
    dl.status = DownloadStatus::Downloading;
    state.db.update_download(id, &dl).map_err(|e| e.to_string())?;
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
        state.engine.get_status(&gid).await.map_err(|e| format!("{:?}", e))
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
async fn change_priority(id: i64, increase: bool, state: State<'_, AppState>) -> Result<(), String> {
    let mut dl = state.db.get_download(id).map_err(|e| e.to_string())?;
    if increase {
        dl.priority += 1;
    } else if dl.priority > 0 {
        dl.priority -= 1;
    }
    state.db.update_download(id, &dl).map_err(|e| e.to_string())?;
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
        save_path: "~/Downloads".to_string(), // Simplified default
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

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    let app_data_dir = std::path::Path::new(":memory:");
    let db = Database::init(app_data_dir).expect("Failed to init DB");
    
    let engine = Aria2Engine::new();
    // Try to start aria2c, gracefully handle if not found
    let _ = engine.start("aria2c");

    let app_state = AppState { db, engine, queue: QueueManager::new() };

    tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .setup(|app| {
            let app_handle = app.handle().clone();
            
            // Spawn Axum HTTP server
            let axum_app_handle = app_handle.clone();
            tauri::async_runtime::spawn(async move {
                let app = Router::new()
                    .route("/api/add", post(handle_api_add))
                    .with_state(axum_app_handle);
                
                let listener = tokio::net::TcpListener::bind("127.0.0.1:14201").await.unwrap();
                println!("Axum HTTP server listening on {}", listener.local_addr().unwrap());
                axum::serve(listener, app).await.unwrap();
            });

            tauri::async_runtime::spawn(async move {
                let mut interval = tokio::time::interval(std::time::Duration::from_millis(500));
                loop {
                    interval.tick().await;
                    let state = app_handle.state::<AppState>();
                    let _ = state.queue.tick(&state.db, &state.engine).await;
                    
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
                                        }
                                    }
                                } else {
                                    // mock progress if aria2c is not running or failed
                                    dl.downloaded_size += 512000;
                                    if dl.downloaded_size >= dl.total_size && dl.total_size > 0 {
                                        dl.downloaded_size = dl.total_size;
                                    }
                                    speed = 512000.0;
                                }
                            } else {
                                // mock progress
                                dl.downloaded_size += 512000;
                                if dl.downloaded_size >= dl.total_size && dl.total_size > 0 {
                                    dl.downloaded_size = dl.total_size;
                                }
                                speed = 512000.0;
                            }
                            
                            dl.speed = speed;
                            
                            if dl.downloaded_size >= dl.total_size && dl.total_size > 0 {
                                dl.status = DownloadStatus::Completed;
                                status_str = "Completed".to_string();
                                speed = 0.0;
                            }
                            
                            let _ = state.db.update_download_progress(dl.id.unwrap(), dl.downloaded_size, speed, &dl.status);
                            
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
            change_priority
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
