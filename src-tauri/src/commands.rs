use crate::download::queue::{validate_schedule, ScheduleOptions};
use crate::log_buffer;
use crate::settings::Settings;
use crate::storage::models::{Download, DownloadFilter, DownloadStatus};
use crate::util::{
    app_data_dir, copy_file_exclusive, full_file_path, lock_or_recover, resolve_download_target,
    sanitize_filename, validate_open_path, LEGACY_DEFAULT_API_TOKEN,
};
use crate::{
    current_settings, enqueue_download, resolve_download_save_path, AppState,
    ExternalDownloadPayload,
};
use tauri::{AppHandle, State};

#[tauri::command]
#[allow(clippy::too_many_arguments)]
pub async fn add_download(
    app: AppHandle,
    url: String,
    filename: String,
    save_path: String,
    referrer: Option<String>,
    user_agent: Option<String>,
    cookies: Option<String>,
    cookie_url: Option<String>,
    state: State<'_, AppState>,
) -> Result<Download, String> {
    let id = enqueue_download(
        &app,
        ExternalDownloadPayload {
            url,
            filename: Some(filename),
            referrer,
            user_agent,
            cookies,
            cookie_url,
            title: None,
            format: None,
            save_path: Some(save_path),
        },
    )
    .await?;
    state.db.get_download(id).map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn pause_download(id: i64, state: State<'_, AppState>) -> Result<(), String> {
    let dl = state.db.get_download(id).map_err(|e| e.to_string())?;
    if let Some(gid) = &dl.aria2_gid {
        state.engine.pause(gid).await.map_err(|e| e.to_string())?;
    }
    // Cancels HLS and yt-dlp tasks (shared watch map)
    if !state.queue.cancel_and_wait_stream(id).await {
        return Err("Download is still stopping; try pausing it again shortly".into());
    }
    if !state
        .db
        .set_status_error_if_current(
            id,
            &[
                DownloadStatus::Queued,
                DownloadStatus::Downloading,
                DownloadStatus::Merging,
                DownloadStatus::Paused,
            ],
            &DownloadStatus::Paused,
            None,
            Some(0.0),
        )
        .map_err(|e| e.to_string())?
    {
        return Err("Download state changed before pause".into());
    }
    Ok(())
}

#[tauri::command]
pub async fn resume_download(id: i64, state: State<'_, AppState>) -> Result<(), String> {
    let dl = state.db.get_download(id).map_err(|e| e.to_string())?;
    if matches!(
        dl.status,
        DownloadStatus::Completed | DownloadStatus::Downloading | DownloadStatus::Merging
    ) {
        return Err("Download is already active or completed".into());
    }
    if dl.status == DownloadStatus::Queued {
        return Ok(());
    }
    if !state
        .db
        .resume_if_current(id, &[DownloadStatus::Paused, DownloadStatus::Failed])
        .map_err(|e| e.to_string())?
    {
        return Err("Download state changed before resume".into());
    }
    Ok(())
}

#[tauri::command]
pub async fn remove_download(
    id: i64,
    delete_file: Option<bool>,
    state: State<'_, AppState>,
) -> Result<(), String> {
    let dl = state.db.get_download(id).map_err(|e| e.to_string())?;
    if let Some(gid) = &dl.aria2_gid {
        let _ = state.engine.remove(gid).await;
    }
    if !state.queue.cancel_and_wait_stream(id).await {
        return Err("Download is still stopping; try removing it again shortly".into());
    }
    // ponytail: optionally delete the downloaded file from disk (default: keep,
    // preserving the user's data). Best-effort — a missing/unwritable file must
    // not block DB row removal.
    if delete_file.unwrap_or(false) {
        let file_path = full_file_path(&dl.save_path, &dl.filename);
        if file_path.exists() {
            if let Err(e) = std::fs::remove_file(&file_path) {
                log::warn!("remove_download: could not delete {}: {}", file_path.display(), e);
            }
        }
    }
    if let Some(id) = dl.id {
        if let Some(part) = crate::download::http::part_path_for(&dl.save_path, &dl.filename, id) {
            let _ = std::fs::remove_file(part);
        }
    }
    state.db.delete_download(id).map_err(|e| e.to_string())?;
    Ok(())
}

#[tauri::command]
pub async fn get_downloads(
    mut filter: DownloadFilter,
    limit: Option<i64>,
    offset: Option<i64>,
    state: State<'_, AppState>,
) -> Result<Vec<Download>, String> {
    // Explicit pagination boundary: callers may request at most one page.
    filter.limit = Some(limit.or(filter.limit).unwrap_or(200).clamp(1, 200));
    filter.offset = Some(offset.or(filter.offset).unwrap_or(0).max(0));
    state.db.get_downloads(&filter).map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn get_download(id: i64, state: State<'_, AppState>) -> Result<Download, String> {
    state.db.get_download(id).map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn get_download_status(
    id: i64,
    state: State<'_, AppState>,
) -> Result<serde_json::Value, String> {
    let dl = state.db.get_download(id).map_err(|e| e.to_string())?;
    if let Some(gid) = dl.aria2_gid {
        if state.engine.is_running() {
            return state.engine.get_status(&gid).await.map_err(|e| e.to_string());
        }
    }
    Ok(serde_json::json!({
        "status": dl.status.as_str(),
        "completedLength": dl.downloaded_size.to_string(),
        "totalLength": dl.total_size.to_string(),
        "downloadSpeed": dl.speed.to_string(),
    }))
}

#[tauri::command]
pub fn set_schedule(
    start_time: Option<String>,
    stop_time: Option<String>,
    active: bool,
    state: State<'_, AppState>,
) -> Result<(), String> {
    let options =
        ScheduleOptions { start_time: start_time.clone(), stop_time: stop_time.clone(), active };
    validate_schedule(&options)?;
    let mut settings = Settings::load(&app_data_dir());
    settings.schedule_active = active;
    settings.schedule_start = start_time;
    settings.schedule_stop = stop_time;
    settings.save(&app_data_dir())?;
    state.queue.set_schedule(options);
    Ok(())
}

#[tauri::command]
pub fn get_schedule(state: State<'_, AppState>) -> Result<ScheduleOptions, String> {
    Ok(state.queue.get_schedule())
}

#[tauri::command]
pub async fn open_folder(path: String) -> Result<(), String> {
    let safe = validate_open_path(&path)?;
    #[cfg(target_os = "macos")]
    {
        let status = std::process::Command::new("open").arg("-R").arg(&safe).spawn();
        if let Err(e) = status {
            return Err(format!("Failed to open folder: {e}"));
        }
        Ok(())
    }
    #[cfg(not(target_os = "macos"))]
    {
        // ponytail: previously a silent no-op on non-macOS (returned Ok(()) but
        // did nothing). Falcon DM is macOS-only; surface an explicit error so the
        // UI doesn't mislead the user into thinking it worked.
        Err("Opening folders is only supported on macOS.".into())
    }
}

#[tauri::command]
pub async fn open_file(path: String) -> Result<(), String> {
    let safe = validate_open_path(&path)?;
    #[cfg(target_os = "macos")]
    {
        let status = std::process::Command::new("open").arg(&safe).spawn();
        if let Err(e) = status {
            return Err(format!("Failed to open file: {e}"));
        }
        Ok(())
    }
    #[cfg(not(target_os = "macos"))]
    {
        Err("Opening files is only supported on macOS.".into())
    }
}

#[tauri::command]
pub async fn change_priority(
    id: i64,
    increase: bool,
    state: State<'_, AppState>,
) -> Result<(), String> {
    if state.db.adjust_priority(id, increase).map_err(|e| e.to_string())? {
        Ok(())
    } else {
        Err("Download not found".into())
    }
}

#[tauri::command]
pub fn get_settings(state: State<'_, AppState>) -> Result<Settings, String> {
    let mut settings = current_settings(&state);
    settings.api_token.clear();
    for profile in &mut settings.download_profiles {
        if profile.cookies.as_deref().is_some_and(|c| !c.trim().is_empty()) {
            profile.cookies = None;
        }
    }
    Ok(settings)
}

/// Extension pairing status — exposes NO secret token to the frontend.
/// The frontend uses this to render onboarding/settings UI; the token only
/// ever leaves the app via the authenticated `/api/pair` HTTP flow after the
/// user explicitly approves a specific extension ID.
#[derive(serde::Serialize)]
pub(crate) struct ExtensionStatus {
    has_token: bool,
    approved_extension_ids: Vec<String>,
    pending_pair_id: Option<String>,
}

#[tauri::command]
pub fn get_extension_status(state: State<'_, AppState>) -> Result<ExtensionStatus, String> {
    let token = lock_or_recover(&state.api_token).clone();
    let has_token = !token.trim().is_empty() && token != LEGACY_DEFAULT_API_TOKEN;
    let approved = lock_or_recover(&state.settings).allowed_extension_ids.clone();
    Ok(ExtensionStatus {
        has_token,
        approved_extension_ids: approved,
        pending_pair_id: lock_or_recover(&state.pending_pair_ids).front().cloned(),
    })
}

#[tauri::command]
pub fn reset_extension_pin(state: State<'_, AppState>) -> Result<(), String> {
    let mut settings = current_settings(&state);
    settings.allowed_extension_ids.clear();
    lock_or_recover(&state.pending_pair_ids).clear();
    settings.save(&app_data_dir())?;
    *lock_or_recover(&state.settings) = settings;
    Ok(())
}

#[tauri::command]
pub fn get_pending_pair(state: State<'_, AppState>) -> Result<Option<String>, String> {
    Ok(lock_or_recover(&state.pending_pair_ids).front().cloned())
}

#[tauri::command]
pub fn get_pending_pairs(state: State<'_, AppState>) -> Result<Vec<String>, String> {
    Ok(lock_or_recover(&state.pending_pair_ids).iter().cloned().collect())
}

/// ponytail: surface the in-memory log ring buffer to the frontend "Logs" panel.
/// Optional level filter narrows the snapshot (e.g. only WARN+ERROR).
#[tauri::command]
pub fn get_logs(level: Option<String>) -> Result<Vec<log_buffer::LogEntry>, String> {
    let snap = log_buffer::snapshot();
    match level.as_deref() {
        Some(filter) if !filter.is_empty() => {
            Ok(snap.into_iter().filter(|e| e.level == filter).collect())
        }
        _ => Ok(snap),
    }
}

#[tauri::command]
pub fn clear_logs() -> Result<(), String> {
    log_buffer::clear();
    Ok(())
}

/// ponytail: aggregate statistics for the Stats panel — counts by status, total
/// downloaded bytes, current aggregate speed. Computed on-demand from the DB so
/// it's always consistent with the list (no separate stats table to sync).
#[derive(serde::Serialize)]
pub(crate) struct DownloadStats {
    active: u64,
    queued: u64,
    paused: u64,
    completed: u64,
    failed: u64,
    total_downloaded_bytes: u64,
    current_speed: f64,
}

#[tauri::command]
pub async fn get_stats(state: State<'_, AppState>) -> Result<DownloadStats, String> {
    let (active, queued, paused, completed, failed, total_bytes, speed) =
        state.db.download_stats().map_err(|e| e.to_string())?;
    Ok(DownloadStats {
        active,
        queued,
        paused,
        completed,
        failed,
        total_downloaded_bytes: total_bytes,
        current_speed: speed,
    })
}

/// ponytail: rename and/or move a completed download's file. The new filename is
/// sanitized; the destination folder is validated against path traversal. Runs
/// the fs op on a blocking thread so the async runtime isn't stalled. DB row is
/// updated only after a successful move so it never points at a missing file.
#[tauri::command]
pub async fn move_download(
    id: i64,
    new_filename: Option<String>,
    new_save_path: Option<String>,
    state: State<'_, AppState>,
) -> Result<(), String> {
    let mut dl = state.db.get_download(id).map_err(|e| e.to_string())?;
    if !matches!(dl.status, DownloadStatus::Completed | DownloadStatus::Failed) {
        return Err("Only completed or failed downloads can be moved".into());
    }

    let filename = match new_filename.as_deref().filter(|f| !f.trim().is_empty()) {
        Some(f) => {
            let sanitized = sanitize_filename(f);
            if sanitized != f.trim() {
                return Err("Invalid download filename".into());
            }
            sanitized
        }
        _ => dl.filename.clone(),
    };
    let requested_dir =
        new_save_path.as_deref().filter(|path| !path.trim().is_empty()).unwrap_or(&dl.save_path);
    let dest_dir = resolve_download_save_path(Some(requested_dir), &dl.category)?;
    let dest_path = resolve_download_target(&dest_dir, &filename)?;
    let src = resolve_download_target(&dl.save_path, &dl.filename)?;

    if std::fs::symlink_metadata(&src).map_err(|e| e.to_string())?.file_type().is_symlink() {
        return Err("Source file cannot be a symlink".into());
    }

    if src == dest_path {
        return Ok(()); // nothing to do
    }
    let dest_save_path = dest_path
        .parent()
        .ok_or_else(|| "Moved file has no parent directory".to_string())?
        .to_string_lossy()
        .to_string();

    // ponytail: blocking fs move on a pool thread.
    let result = tokio::task::spawn_blocking(move || copy_file_exclusive(&src, &dest_path))
        .await
        .map_err(|e| format!("move task failed: {e}"))?;

    result.map_err(|e| format!("failed to move file: {e}"))?;

    // Update DB only after the file landed.
    dl.filename = filename;
    dl.save_path = dest_save_path;
    state.db.update_download(id, &dl).map_err(|e| e.to_string())?;
    Ok(())
}

/// ponytail: toggle the archived flag on a download. Archived downloads are
/// hidden from the active list (they don't match status/category filters) but
/// remain in the DB and on disk. Unarchiving restores them.
#[tauri::command]
pub async fn archive_download(
    id: i64,
    archived: bool,
    state: State<'_, AppState>,
) -> Result<(), String> {
    let terminal = [DownloadStatus::Completed, DownloadStatus::Failed];
    if state.db.set_archived_if_status(id, archived, &terminal).map_err(|e| e.to_string())? {
        Ok(())
    } else {
        Err("Only completed or failed downloads can be archived".into())
    }
}

#[tauri::command]
pub fn approve_extension_pair(
    extension_id: String,
    state: State<'_, AppState>,
) -> Result<(), String> {
    let id = extension_id.trim();
    if id.is_empty() {
        return Err("empty extension id".into());
    }
    let mut settings = current_settings(&state);
    if !settings.allowed_extension_ids.iter().any(|x| x == id) {
        settings.allowed_extension_ids.push(id.to_string());
    }
    settings.save(&app_data_dir())?;
    *lock_or_recover(&state.settings) = settings;
    lock_or_recover(&state.pending_pair_ids).retain(|pending| pending != id);
    Ok(())
}

#[tauri::command]
pub async fn save_settings(settings: Settings, state: State<'_, AppState>) -> Result<(), String> {
    let dir = app_data_dir();
    let current = current_settings(&state);
    let mut settings = settings;
    if settings.api_token.trim().is_empty() || settings.api_token == LEGACY_DEFAULT_API_TOKEN {
        settings.api_token = current.api_token.clone();
    }
    for profile in &mut settings.download_profiles {
        if profile.cookies.as_deref().map(str::trim).unwrap_or("").is_empty() {
            profile.cookies = current
                .download_profiles
                .iter()
                .find(|old| old.name == profile.name && old.url_pattern == profile.url_pattern)
                .and_then(|old| old.cookies.clone());
        }
    }
    settings.save(&dir)?;

    state.queue.set_concurrent_downloads(settings.max_concurrent_downloads as usize);
    state.queue.set_max_connections(settings.max_connections_per_server as usize);
    state.queue.set_http_options(settings.proxy.clone(), settings.speed_limit_kbps);
    *lock_or_recover(&state.api_token) = {
        let t = settings.api_token.trim();
        if t.is_empty() || t == LEGACY_DEFAULT_API_TOKEN {
            let fresh = uuid::Uuid::new_v4().to_string();
            let mut s = settings.clone();
            s.api_token = fresh.clone();
            let _ = s.save(&dir);
            *lock_or_recover(&state.settings) = s;
            fresh
        } else {
            *lock_or_recover(&state.settings) = settings.clone();
            settings.api_token.clone()
        }
    };

    if state.engine.is_running() {
        let _ = state.engine.apply_speed_limit(settings.speed_limit_kbps).await;
        let _ = state.engine.apply_proxy(settings.proxy.as_deref()).await;
    }

    Ok(())
}

#[tauri::command]
pub fn install_native_host_manifests(
    chrome_extension_id: String,
    edge_extension_id: Option<String>,
) -> Result<(), String> {
    if !crate::extension_host::native_host_install_supported() {
        return Err("Native host install is not supported on this platform".to_string());
    }
    let chrome = chrome_extension_id.trim().to_string();
    let edge = edge_extension_id
        .map(|id| id.trim().to_string())
        .filter(|id| !id.is_empty())
        .unwrap_or_else(|| chrome.clone());
    let executable = crate::extension_host::resolve_native_host_binary().ok_or_else(|| {
        "Native host binary not found. Build it with: cargo build --bin falcon-dm-native-host"
            .to_string()
    })?;
    crate::extension_host::install_native_host_manifests(&executable, &chrome, &edge)
}
