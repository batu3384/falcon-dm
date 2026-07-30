pub mod storage;
pub mod download;

use download::engine::{Aria2Engine, Aria2Options};
use storage::models::{Download, DownloadFilter, DownloadStatus, DownloadCategory};
use storage::Database;
use tauri::State;
use chrono::Utc;

pub struct AppState {
    pub db: Database,
    pub engine: Aria2Engine,
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
        total_size: 0,
        downloaded_size: 0,
        status: DownloadStatus::Queued,
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

    let opts = Aria2Options {
        dir: save_path.clone(),
        filename: filename.clone(),
        split: 16,
        max_connections: 16,
        headers: vec![],
        referrer: None,
        user_agent: None,
    };

    if let Ok(gid) = state.engine.add_download(&url, opts).await {
        dl.aria2_gid = Some(gid);
        let _ = state.db.update_download(id, &dl);
    }

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

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    let app_data_dir = std::path::Path::new(":memory:");
    let db = Database::init(app_data_dir).expect("Failed to init DB");
    
    let engine = Aria2Engine::new();
    // Try to start aria2c, gracefully handle if not found
    let _ = engine.start("aria2c");

    let app_state = AppState { db, engine };

    tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .manage(app_state)
        .invoke_handler(tauri::generate_handler![
            greet,
            add_download,
            pause_download,
            resume_download,
            remove_download,
            get_downloads,
            get_download_status
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
