use crate::download::queue::{validate_schedule, ScheduleOptions, StopReason};
use crate::log_buffer;
use crate::pairing::PendingPair;
use crate::settings::Settings;
use crate::storage::models::{Download, DownloadFilter, DownloadStatus};
use crate::util::{
    app_data_dir, free_disk_space, full_file_path, is_junk_media_url, lock_or_recover,
    move_file_exclusive, normalize_media_url, resolve_download_target, sanitize_filename,
    validate_enqueue_url, validate_open_path, LEGACY_DEFAULT_API_TOKEN,
};
use crate::{
    canonical_store_url, current_settings, emit_row, enqueue_download, parse_extension_id,
    remove_download_artifacts, resolve_download_save_path, AppState, ExternalDownloadPayload,
};
use tauri::{AppHandle, Emitter, State};

#[tauri::command]
#[allow(clippy::too_many_arguments)]
pub async fn add_download(
    app: AppHandle,
    url: String,
    filename: Option<String>,
    save_path: Option<String>,
    referrer: Option<String>,
    user_agent: Option<String>,
    cookies: Option<String>,
    cookie_url: Option<String>,
    format: Option<String>,
    expected_hash: Option<String>,
    state: State<'_, AppState>,
) -> Result<Download, String> {
    let blank = |value: Option<String>| value.filter(|v| !v.trim().is_empty());
    let id = enqueue_download(
        &app,
        ExternalDownloadPayload {
            url,
            filename: blank(filename),
            referrer: blank(referrer),
            user_agent: blank(user_agent),
            cookies: blank(cookies),
            cookie_url: blank(cookie_url),
            title: None,
            format: blank(format),
            save_path: blank(save_path),
            expected_hash: blank(expected_hash),
            subtitle_tracks: None,
        },
    )
    .await?;
    state.db.get_download(id).map_err(|e| e.to_string())
}

/// Shared pause implementation for IPC and the local API. The emit callback
/// keeps the core testable while mirroring `QueueManager::pause`'s
/// stop/update/flush order exactly.
pub(crate) async fn pause_download_core<F>(
    state: &AppState,
    id: i64,
    mut emit: F,
) -> Result<(), String>
where
    F: FnMut(i64),
{
    let row = state.db.get_download(id).map_err(|e| e.to_string())?;
    if row.status == DownloadStatus::Paused {
        return Ok(());
    }
    state.queue.request_stop(id, StopReason::Pause);
    let paused = state
        .db
        .set_status_error_if_current(
            id,
            &[DownloadStatus::Queued, DownloadStatus::Downloading, DownloadStatus::Merging],
            &DownloadStatus::Paused,
            None,
            Some(0.0),
        )
        .unwrap_or(false);
    if paused {
        emit(id);
    }
    // Let the worker flush its resume state before the caller reports success.
    state.queue.stop_and_wait(id, StopReason::Pause).await;
    if paused {
        Ok(())
    } else {
        Err("Download state changed before pause".into())
    }
}

#[tauri::command]
pub async fn pause_download(
    app: AppHandle,
    id: i64,
    state: State<'_, AppState>,
) -> Result<(), String> {
    pause_download_core(&state, id, |id| emit_row(&app, &state.db, id)).await
}

pub(crate) fn resume_download_core<F>(state: &AppState, id: i64, mut emit: F) -> Result<(), String>
where
    F: FnMut(i64),
{
    let dl = state.db.get_download(id).map_err(|e| e.to_string())?;
    match dl.status {
        DownloadStatus::Completed => return Err("Download is already completed".into()),
        // Already on its way (or waiting out a retry backoff: start it now).
        DownloadStatus::Downloading | DownloadStatus::Merging => return Ok(()),
        DownloadStatus::Queued => {
            let _ = state.db.clear_retry_backoff(id);
            emit(id);
            return Ok(());
        }
        DownloadStatus::Paused | DownloadStatus::Failed => {}
    }
    // Partial data stays on disk: a failed download continues where it stopped.
    if !state
        .db
        .resume_if_current(id, &[DownloadStatus::Paused, DownloadStatus::Failed])
        .map_err(|e| e.to_string())?
    {
        return Err("Download state changed before resume".into());
    }
    emit(id);
    Ok(())
}

#[tauri::command]
pub async fn resume_download(
    app: AppHandle,
    id: i64,
    state: State<'_, AppState>,
) -> Result<(), String> {
    resume_download_core(&state, id, |id| emit_row(&app, &state.db, id))
}

#[tauri::command]
pub async fn pause_all_downloads(app: AppHandle) -> Result<usize, String> {
    Ok(crate::pause_all(&app).await)
}

#[tauri::command]
pub async fn resume_all_downloads(app: AppHandle) -> Result<usize, String> {
    Ok(crate::resume_all(&app))
}

pub(crate) async fn remove_download_core<F>(
    state: &AppState,
    id: i64,
    delete_file: bool,
    mut emit_removed: F,
) -> Result<(), String>
where
    F: FnMut(i64),
{
    state.db.get_download(id).map_err(|e| e.to_string())?;
    // Keep the row out of queue claims until the row delete has fully finished.
    state.queue.begin_remove(id);
    let result: Result<(), String> = async {
        if !state.queue.stop_and_wait(id, StopReason::Remove).await {
            Err("Download is still stopping; try removing it again shortly".to_string())?;
        }
        // Read again: the worker may have renamed the file while finishing.
        let dl = state.db.get_download(id).map_err(|e| e.to_string())?;
        if delete_file && dl.status == DownloadStatus::Completed {
            let file_path = resolve_download_target(&dl.save_path, &dl.filename)?;
            if file_path.exists() {
                std::fs::remove_file(&file_path)
                    .map_err(|e| format!("Could not delete file from disk: {e}"))?;
            }
        }
        remove_download_artifacts(&dl.save_path, &dl.filename, id);
        state.db.delete_download(id).map_err(|e| e.to_string())?;
        emit_removed(id);
        Ok(())
    }
    .await;
    state.queue.end_remove(id);
    result
}

#[tauri::command]
pub async fn remove_download(
    app: AppHandle,
    id: i64,
    delete_file: Option<bool>,
    state: State<'_, AppState>,
) -> Result<(), String> {
    remove_download_core(&state, id, delete_file.unwrap_or(false), |id| {
        let _ = app.emit("download-removed", id);
    })
    .await
}

/// Shared list boundary: callers may request at most one bounded page, and
/// expensive open-ended search/status input is clipped before reaching SQL.
pub(crate) fn bounded_download_filter(
    mut filter: DownloadFilter,
    limit: Option<i64>,
    offset: Option<i64>,
) -> DownloadFilter {
    filter.limit = Some(limit.or(filter.limit).unwrap_or(200).clamp(1, 200));
    filter.offset = Some(offset.or(filter.offset).unwrap_or(0).max(0));
    if let Some(search) = filter.search.take() {
        let clipped: String = search.chars().take(200).collect();
        filter.search = if clipped.trim().is_empty() { None } else { Some(clipped) };
    }
    if let Some(ref mut statuses) = filter.statuses {
        statuses.truncate(8);
    }
    filter
}

#[tauri::command]
pub async fn get_downloads(
    filter: DownloadFilter,
    limit: Option<i64>,
    offset: Option<i64>,
    state: State<'_, AppState>,
) -> Result<Vec<Download>, String> {
    let filter = bounded_download_filter(filter, limit, offset);
    state.db.get_downloads(&filter).map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn get_download(id: i64, state: State<'_, AppState>) -> Result<Download, String> {
    state.db.get_download(id).map_err(|e| e.to_string())
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
    let mut settings = current_settings(&state);
    settings.schedule_active = active;
    settings.schedule_start = start_time;
    settings.schedule_stop = stop_time;
    settings.save(&app_data_dir())?;
    *lock_or_recover(&state.settings) = settings;
    state.queue.set_schedule(options);
    Ok(())
}

#[tauri::command]
pub fn get_schedule(state: State<'_, AppState>) -> Result<ScheduleOptions, String> {
    Ok(state.queue.get_schedule())
}

/// Exact file location of every download row, archived or not. `open` on a
/// recorded path is legitimate even when the save dir lives outside the
/// download roots (custom per-download or category folder on any volume).
fn recorded_download_files(db: &crate::storage::Database) -> Vec<std::path::PathBuf> {
    let mut files = Vec::new();
    for archived in [None, Some(true)] {
        if let Ok(rows) = db.get_downloads(&DownloadFilter { archived, ..Default::default() }) {
            files.extend(rows.iter().map(|row| full_file_path(&row.save_path, &row.filename)));
        }
    }
    files
}

#[tauri::command]
pub async fn open_folder(path: String, state: State<'_, AppState>) -> Result<(), String> {
    let safe = validate_open_path(&path, &recorded_download_files(&state.db))?;
    #[cfg(target_os = "macos")]
    {
        // Reveal the file when it exists, else open the folder it will land in.
        let mut command = std::process::Command::new("open");
        if safe.is_file() {
            command.arg("-R").arg(&safe);
        } else if safe.is_dir() {
            command.arg(&safe);
        } else {
            command.arg(safe.parent().ok_or_else(|| "Path does not exist".to_string())?);
        }
        command.spawn().map_err(|e| format!("Failed to open folder: {e}"))?;
        Ok(())
    }
    #[cfg(not(target_os = "macos"))]
    {
        let _ = safe;
        Err("Opening folders is only supported on macOS.".into())
    }
}

#[tauri::command]
pub async fn open_file(path: String, state: State<'_, AppState>) -> Result<(), String> {
    let safe = validate_open_path(&path, &recorded_download_files(&state.db))?;
    if !safe.is_file() {
        return Err("ERR_FILE_MISSING".into());
    }
    #[cfg(target_os = "macos")]
    {
        std::process::Command::new("open")
            .arg(&safe)
            .spawn()
            .map_err(|e| format!("Failed to open file: {e}"))?;
        Ok(())
    }
    #[cfg(not(target_os = "macos"))]
    {
        Err("Opening files is only supported on macOS.".into())
    }
}

#[tauri::command]
pub async fn change_priority(
    app: AppHandle,
    id: i64,
    increase: bool,
    state: State<'_, AppState>,
) -> Result<(), String> {
    if state.db.adjust_priority(id, increase).map_err(|e| e.to_string())? {
        emit_row(&app, &state.db, id);
        Ok(())
    } else {
        Err("Download not found".into())
    }
}

const MAX_PER_DOWNLOAD_SPEED_LIMIT_KBPS: u32 = 1_048_576;

/// Persist a row cap, then adjust the live worker when one exists. The order
/// closes the startup race: a job begun before the DB write is already in the
/// active map, and a job begun after it reads the new value from its row.
pub(crate) fn update_download_speed_limit(
    db: &crate::storage::Database,
    queue: &crate::download::queue::QueueManager,
    id: i64,
    speed_limit_kbps: u32,
) -> Result<bool, String> {
    let kbps = speed_limit_kbps.min(MAX_PER_DOWNLOAD_SPEED_LIMIT_KBPS);
    let updated = db.set_speed_limit(id, kbps).map_err(|e| e.to_string())?;
    if updated {
        queue.set_download_speed_limit(id, kbps);
    }
    Ok(updated)
}

/// Set one download's cap in decimal KB/s; zero means the global cap. Native
/// HTTP/HLS workers pick this up live through their shared row bucket. The
/// `--limit-rate` is fixed when its child process starts, so there it applies
/// to the next start or retry.
#[tauri::command]
pub async fn set_download_speed_limit(
    app: AppHandle,
    id: i64,
    speed_limit_kbps: u32,
    state: State<'_, AppState>,
) -> Result<(), String> {
    if update_download_speed_limit(&state.db, &state.queue, id, speed_limit_kbps)? {
        emit_row(&app, &state.db, id);
        Ok(())
    } else {
        Err("Download not found".into())
    }
}

#[tauri::command]
pub fn get_settings(state: State<'_, AppState>) -> Result<Settings, String> {
    let mut settings = current_settings(&state);
    if settings.load_corrupt {
        return Err("settings.json is corrupt; rename or delete it before saving".into());
    }
    // Secrets never reach the webview.
    settings.api_token.clear();
    for profile in &mut settings.download_profiles {
        if profile.cookies.as_deref().is_some_and(|c| !c.trim().is_empty()) {
            profile.cookies = None;
        }
    }
    Ok(settings)
}

/// Extension pairing status. The API token is never part of it: it only leaves
/// the app through `/api/pair` after the user approved that extension.
#[derive(serde::Serialize)]
pub(crate) struct ExtensionStatus {
    approved_extension_ids: Vec<String>,
    pending: Vec<PendingPair>,
}

#[tauri::command]
pub fn get_extension_status(state: State<'_, AppState>) -> Result<ExtensionStatus, String> {
    Ok(ExtensionStatus {
        approved_extension_ids: lock_or_recover(&state.settings).allowed_extension_ids.clone(),
        pending: lock_or_recover(&state.pairing).pending(),
    })
}

#[tauri::command]
pub fn approve_extension_pair(
    extension_id: String,
    state: State<'_, AppState>,
) -> Result<(), String> {
    let id = parse_extension_id(&extension_id)?;
    // Only a request the user is looking at can be approved: no blind allowlisting.
    if !lock_or_recover(&state.pairing).pending().iter().any(|p| p.extension_id == id) {
        return Err("No pending pairing request for this extension".into());
    }
    let mut settings = current_settings(&state);
    if !settings.allowed_extension_ids.iter().any(|x| x == id) {
        settings.allowed_extension_ids.push(id.to_string());
    }
    settings.save(&app_data_dir())?;
    *lock_or_recover(&state.settings) = settings;
    if !lock_or_recover(&state.pairing).approve(id) {
        return Err("Pairing request expired before approval".into());
    }
    Ok(())
}

#[tauri::command]
pub fn reject_extension_pair(
    extension_id: String,
    state: State<'_, AppState>,
) -> Result<(), String> {
    let id = parse_extension_id(&extension_id)?;
    lock_or_recover(&state.pairing).reject(id);
    Ok(())
}

/// Disconnect one extension. The token is rotated so its cached copy is dead;
/// extensions that stay approved must complete a fresh user-approved pairing.
#[tauri::command]
pub fn revoke_extension(extension_id: String, state: State<'_, AppState>) -> Result<(), String> {
    let id = parse_extension_id(&extension_id)?;
    let mut settings = current_settings(&state);
    settings.allowed_extension_ids.retain(|x| x != id);
    settings.api_token = uuid::Uuid::new_v4().to_string();
    settings.save(&app_data_dir())?;
    *lock_or_recover(&state.api_token) = settings.api_token.clone();
    *lock_or_recover(&state.settings) = settings;
    Ok(())
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

/// Aggregates for the sidebar, status bar and Stats panel. Computed from the DB
/// on demand so they always agree with the list.
#[derive(serde::Serialize)]
pub(crate) struct DownloadStats {
    active: u64,
    queued: u64,
    paused: u64,
    completed: u64,
    failed: u64,
    total_downloaded_bytes: u64,
    current_speed: f64,
    all: u64,
    archived: u64,
    video: u64,
    music: u64,
    document: u64,
    archive: u64,
    program: u64,
    /// Free bytes on the default download volume.
    free_disk_bytes: Option<u64>,
    speed_limit_kbps: u32,
}

pub(crate) fn download_stats_for_state(state: &AppState) -> Result<DownloadStats, String> {
    let stats = state.db.download_stats().map_err(|e| e.to_string())?;
    let root = {
        let settings = lock_or_recover(&state.settings);
        if settings.default_download_path.trim().is_empty() {
            crate::util::default_download_dir()
        } else {
            crate::util::expand_tilde(&settings.default_download_path)
        }
    };
    Ok(DownloadStats {
        active: stats.active,
        queued: stats.queued,
        paused: stats.paused,
        completed: stats.completed,
        failed: stats.failed,
        total_downloaded_bytes: stats.total_downloaded_bytes,
        current_speed: stats.current_speed,
        all: stats.all,
        archived: stats.archived,
        video: stats.video,
        music: stats.music,
        document: stats.document,
        archive: stats.archive,
        program: stats.program,
        free_disk_bytes: free_disk_space(&root),
        speed_limit_kbps: state.queue.speed_limit_kbps(),
    })
}

#[tauri::command]
pub async fn get_stats(state: State<'_, AppState>) -> Result<DownloadStats, String> {
    download_stats_for_state(&state)
}

/// Rename and/or move a finished download. The DB row changes only after the
/// file landed, so it never points at a missing file.
#[tauri::command]
pub async fn move_download(
    app: AppHandle,
    id: i64,
    new_filename: Option<String>,
    new_save_path: Option<String>,
    state: State<'_, AppState>,
) -> Result<(), String> {
    let mut dl = state.db.get_download(id).map_err(|e| e.to_string())?;
    if dl.status != DownloadStatus::Completed {
        return Err("Only completed downloads can be moved".into());
    }

    let filename = match new_filename.as_deref().filter(|f| !f.trim().is_empty()) {
        Some(f) => {
            let sanitized = sanitize_filename(f);
            if sanitized.is_empty() {
                return Err("Invalid download filename".into());
            }
            sanitized
        }
        _ => dl.filename.clone(),
    };
    let requested_dir =
        new_save_path.as_deref().filter(|path| !path.trim().is_empty()).unwrap_or(&dl.save_path);
    let dest_dir = resolve_download_save_path(&state, Some(requested_dir), &dl.category)?;
    let dest_path = resolve_download_target(&dest_dir, &filename)?;
    let src = resolve_download_target(&dl.save_path, &dl.filename)?;

    let meta = std::fs::symlink_metadata(&src).map_err(|_| "ERR_FILE_MISSING".to_string())?;
    if meta.file_type().is_symlink() {
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

    let (from, to) = (src.clone(), dest_path.clone());
    tokio::task::spawn_blocking(move || move_file_exclusive(&from, &to))
        .await
        .map_err(|e| format!("move task failed: {e}"))?
        .map_err(|e| format!("failed to move file: {e}"))?;

    dl.category = crate::storage::models::DownloadCategory::from_filename(&filename);
    dl.filename = filename;
    dl.save_path = dest_save_path;
    if let Err(error) = state.db.update_download(id, &dl) {
        // Put the bytes back so the row still points at a real file.
        let (from, to) = (dest_path.clone(), src.clone());
        let _ = tokio::task::spawn_blocking(move || move_file_exclusive(&from, &to)).await;
        return Err(error.to_string());
    }
    emit_row(&app, &state.db, id);
    Ok(())
}

/// ponytail: toggle the archived flag on a download. Archived downloads are
/// hidden from the active list (they don't match status/category filters) but
/// remain in the DB and on disk. Unarchiving restores them.
#[tauri::command]
pub async fn archive_download(
    app: AppHandle,
    id: i64,
    archived: bool,
    state: State<'_, AppState>,
) -> Result<(), String> {
    let terminal = [DownloadStatus::Completed, DownloadStatus::Failed];
    if state.db.set_archived_if_status(id, archived, &terminal).map_err(|e| e.to_string())? {
        emit_row(&app, &state.db, id);
        Ok(())
    } else {
        Err("Only completed or failed downloads can be archived".into())
    }
}

/// Validate and canonicalize a replacement link exactly like an enqueue:
/// same SSRF rules (literal hosts only, no DNS — refreshing must work
/// offline), same junk-media rejection, same transient-range stripping, one
/// canonical spelling so the resume-state comparison compares equal strings.
fn canonical_refresh_url(raw: &str) -> Result<String, String> {
    if let Err(reason) = validate_enqueue_url(raw) {
        return Err(if reason == "ERR_UNSUPPORTED_MAGNET" { reason } else { "invalid url".into() });
    }
    if is_junk_media_url(raw) {
        return Err("not a real media url".into());
    }
    Ok(canonical_store_url(&normalize_media_url(raw.trim())))
}

fn write_state_bytes(path: &std::path::Path, bytes: &[u8]) -> std::io::Result<()> {
    use std::io::Write;
    let tmp = path.with_extension("state.tmp");
    {
        let mut file = std::fs::File::create(&tmp)?;
        file.write_all(bytes)?;
        file.sync_all()?;
    }
    std::fs::rename(&tmp, path)
}

/// Byte-exact resume only engages when the `.falcon.state` file names the same
/// URL as the row (http.rs `read_state` compares them), so a refresh must
/// carry the state file over to the new link. Only the `url` key is touched;
/// the validator stays — it is what detects a different entity on the new
/// link and makes the engine restart from zero. Returns the original bytes
/// for rollback, `None` when there is nothing usable to preserve.
fn rewrite_state_url(save_path: &str, filename: &str, id: i64, url: &str) -> Option<Vec<u8>> {
    let path = crate::download::http::state_path_for(save_path, filename, id)?;
    let original = std::fs::read(&path).ok()?;
    let mut value: serde_json::Value = serde_json::from_slice(&original).ok()?;
    {
        let object = value.as_object_mut()?;
        match object.get("url").and_then(|existing| existing.as_str()) {
            Some(existing) if existing != url => {
                object.insert("url".into(), serde_json::Value::String(url.to_string()));
            }
            // In sync already — or foreign json the engine would ignore anyway.
            _ => return None,
        }
    }
    let bytes = serde_json::to_vec(&value).ok()?;
    write_state_bytes(&path, &bytes).ok()?;
    Some(original)
}

fn restore_state_file(save_path: &str, filename: &str, id: i64, original: &[u8]) {
    if let Some(path) = crate::download::http::state_path_for(save_path, filename, id) {
        let _ = write_state_bytes(&path, original);
    }
}

/// Enqueue only stores cookies whose origin matches the download, so the row's
/// old URL is the provenance of its session. A refreshed link may keep the
/// cookies only under the same rule — otherwise a cross-host paste would leak
/// the old site's session to an attacker-controlled host.
fn refresh_keeps_cookies(old_url: &str, new_url: &str) -> bool {
    crate::cookie_url_matches_download(new_url, Some(old_url))
}

/// Replace the link of a stopped download with a fresh one. The `.part` file
/// and its resume state are preserved, so a refresh of an expired signed link
/// continues byte-exactly; when the new link serves a different entity, the
/// engine's validator machinery discards the partial data and starts over.
#[tauri::command]
pub async fn refresh_download_url(
    app: AppHandle,
    id: i64,
    new_url: String,
    state: State<'_, AppState>,
) -> Result<(), String> {
    let url = canonical_refresh_url(&new_url)?;
    let row = state.db.get_download(id).map_err(|e| e.to_string())?;
    if !matches!(row.status, DownloadStatus::Failed | DownloadStatus::Paused) {
        return Err("Only failed or paused downloads can be refreshed".into());
    }
    // A freshly paused row can still have a worker flushing its final resume
    // state; rewriting underneath it would be overwritten with the old URL.
    // Wait for the slot to release — never signal it: a download the user
    // just resumed must keep running (the gate below rejects it instead).
    let drained = tokio::time::timeout(std::time::Duration::from_secs(5), async {
        while state.queue.is_active(id) {
            tokio::time::sleep(std::time::Duration::from_millis(25)).await;
        }
    })
    .await
    .is_ok();
    if !drained {
        return Err("Download is still stopping; try refreshing it again shortly".into());
    }
    // State first: a stopped row cannot start before the requeue below, so no
    // worker ever reads a half-migrated pair. The atomic update is the gate —
    // when it loses a race, the state file goes back byte-exactly.
    let backup = rewrite_state_url(&row.save_path, &row.filename, id, &url);
    if !state
        .db
        .refresh_url_if_stopped(id, &url, refresh_keeps_cookies(&row.url, &url))
        .map_err(|e| e.to_string())?
    {
        if let Some(original) = backup {
            restore_state_file(&row.save_path, &row.filename, id, &original);
        }
        return Err("Download state changed before refresh".into());
    }
    emit_row(&app, &state.db, id);
    Ok(())
}

fn persist_settings_in_dir(
    state: &AppState,
    settings: Settings,
    app_dir: &std::path::Path,
) -> Result<(), String> {
    let current = current_settings(state);
    if current.load_corrupt {
        return Err("settings.json is corrupt; rename or delete it before saving".into());
    }
    let mut settings = settings;
    // The UI never sees or edits these: token, allowlist, schedule.
    settings.api_token = current.api_token.clone();
    if settings.api_token.trim().is_empty() || settings.api_token == LEGACY_DEFAULT_API_TOKEN {
        settings.api_token = uuid::Uuid::new_v4().to_string();
    }
    settings.restore_blank_profile_cookies(&current.download_profiles);
    settings.preserve_command_owned_fields(&current);
    let live = state.queue.get_schedule();
    settings.apply_live_schedule(live.active, live.start_time.clone(), live.stop_time.clone());
    settings.clamp_runtime_bounds();
    settings.save(app_dir)?;

    state.queue.apply_settings(&settings);
    *lock_or_recover(&state.api_token) = settings.api_token.clone();
    *lock_or_recover(&state.settings) = settings;
    Ok(())
}

fn update_global_speed_limit_in_dir(
    state: &AppState,
    speed_limit_kbps: u32,
    app_dir: &std::path::Path,
) -> Result<u32, String> {
    let mut settings = current_settings(state);
    settings.speed_limit_kbps = speed_limit_kbps;
    persist_settings_in_dir(state, settings, app_dir)?;
    Ok(state.queue.speed_limit_kbps())
}

/// Change the global cap through the same preservation/clamping/save pipeline
/// as a Settings-page write, then apply it to the live token bucket.
pub(crate) fn update_global_speed_limit(
    state: &AppState,
    speed_limit_kbps: u32,
) -> Result<u32, String> {
    update_global_speed_limit_in_dir(state, speed_limit_kbps, &app_data_dir())
}

#[tauri::command]
pub async fn save_settings(settings: Settings, state: State<'_, AppState>) -> Result<(), String> {
    persist_settings_in_dir(&state, settings, &app_data_dir())
}

#[derive(serde::Serialize)]
pub struct UpdateCheckResult {
    pub current_version: String,
    pub latest_version: Option<String>,
    pub update_available: bool,
    pub release_url: Option<String>,
}

fn version_newer(latest: &str, current: &str) -> bool {
    let parse = |raw: &str| -> Vec<u32> {
        raw.trim()
            .trim_start_matches(['v', 'V'])
            .split('.')
            .map(|part| part.parse::<u32>().unwrap_or(0))
            .collect()
    };
    let latest_parts = parse(latest);
    let current_parts = parse(current);
    let width = latest_parts.len().max(current_parts.len()).max(3);
    for index in 0..width {
        let l = *latest_parts.get(index).unwrap_or(&0);
        let c = *current_parts.get(index).unwrap_or(&0);
        if l > c {
            return true;
        }
        if l < c {
            return false;
        }
    }
    false
}

/// Only this repo's GitHub release pages may be handed to the OS opener.
fn github_release_page_url(raw: &str) -> Option<String> {
    let parsed = url::Url::parse(raw.trim()).ok()?;
    if parsed.scheme() != "https" {
        return None;
    }
    if !parsed.username().is_empty() || parsed.password().is_some() {
        return None;
    }
    let host = parsed.host_str()?.to_ascii_lowercase();
    if host != "github.com" && host != "www.github.com" {
        return None;
    }
    let path = parsed.path().to_ascii_lowercase();
    if path != "/batu3384/falcon-dm" && !path.starts_with("/batu3384/falcon-dm/") {
        return None;
    }
    Some(parsed.to_string())
}

fn github_api_redirect(attempt: reqwest::redirect::Attempt<'_>) -> reqwest::redirect::Action {
    if attempt.previous().len() >= 4 {
        return attempt.error("too many redirects");
    }
    match attempt.url().host_str().map(|host| host.to_ascii_lowercase()) {
        Some(host)
            if host == "github.com" || host == "www.github.com" || host == "api.github.com" =>
        {
            attempt.follow()
        }
        _ => attempt.error("redirect blocked"),
    }
}

#[tauri::command]
pub async fn check_for_updates() -> Result<UpdateCheckResult, String> {
    let current = env!("CARGO_PKG_VERSION").to_string();
    let client = reqwest::Client::builder()
        .user_agent("Falcon-DM-Updater")
        .timeout(std::time::Duration::from_secs(12))
        .redirect(reqwest::redirect::Policy::custom(github_api_redirect))
        .build()
        .map_err(|e| e.to_string())?;
    let resp = client
        .get("https://api.github.com/repos/batu3384/falcon-dm/releases/latest")
        .header("Accept", "application/vnd.github+json")
        .send()
        .await
        .map_err(|e| format!("Update check failed: {e}"))?;
    if !resp.status().is_success() {
        return Err(format!("Update check HTTP {}", resp.status().as_u16()));
    }
    #[derive(serde::Deserialize)]
    struct GhRelease {
        tag_name: String,
        html_url: String,
    }
    let release: GhRelease = resp.json().await.map_err(|e| e.to_string())?;
    let latest =
        release.tag_name.trim_start_matches(['v', 'V']).chars().take(32).collect::<String>();
    Ok(UpdateCheckResult {
        update_available: version_newer(&latest, &current),
        current_version: current,
        latest_version: Some(latest),
        release_url: github_release_page_url(&release.html_url),
    })
}

/// WKWebView blocks `navigator.clipboard.readText` outside a user gesture.
/// `pbpaste` reads the pasteboard the toolbar monitor already asked for.
#[tauri::command]
pub async fn read_clipboard() -> Result<String, String> {
    tauri::async_runtime::spawn_blocking(read_clipboard_blocking)
        .await
        .map_err(|e| e.to_string())?
}

fn normalize_pasteboard_text(raw: &str) -> String {
    raw.trim().chars().take(2048).collect()
}

fn read_clipboard_blocking() -> Result<String, String> {
    #[cfg(target_os = "macos")]
    {
        let output = std::process::Command::new("pbpaste").output().map_err(|e| e.to_string())?;
        if !output.status.success() {
            return Err("clipboard read failed".into());
        }
        let text = String::from_utf8_lossy(&output.stdout);
        Ok(normalize_pasteboard_text(&text))
    }
    #[cfg(not(target_os = "macos"))]
    {
        Err("clipboard read is only available on macOS".into())
    }
}

#[cfg(test)]
mod speed_limit_tests {
    use super::update_download_speed_limit;
    use crate::download::queue::QueueManager;
    use crate::storage::models::{Download, DownloadCategory, DownloadStatus};
    use crate::storage::Database;

    fn insert_row(db: &Database, status: DownloadStatus) -> i64 {
        db.insert_download(&Download {
            id: None,
            url: format!("https://example.com/{status}.bin"),
            filename: format!("{status}.bin"),
            save_path: "/tmp".into(),
            total_size: 0,
            downloaded_size: 0,
            status,
            category: DownloadCategory::Other,
            speed: 0.0,
            segments: 1,
            priority: 1,
            created_at: "t".into(),
            completed_at: None,
            error_message: None,
            referrer: None,
            user_agent: None,
            cookies: None,
            archived: false,
            attempts: 0,
            retry_at: None,
            expected_hash: None,
            speed_limit_kbps: None,
            subtitle_tracks: None,
        })
        .unwrap()
    }

    #[test]
    fn speed_limit_command_roundtrip_accepts_every_status_and_zero_means_global() {
        let db = Database::in_memory().unwrap();
        let queue = QueueManager::new();
        for status in [
            DownloadStatus::Queued,
            DownloadStatus::Downloading,
            DownloadStatus::Paused,
            DownloadStatus::Completed,
            DownloadStatus::Failed,
            DownloadStatus::Merging,
        ] {
            let id = insert_row(&db, status);
            assert!(update_download_speed_limit(&db, &queue, id, 512).unwrap());
            assert_eq!(db.get_download(id).unwrap().speed_limit_kbps, Some(512));
            assert!(update_download_speed_limit(&db, &queue, id, 0).unwrap());
            assert_eq!(db.get_download(id).unwrap().speed_limit_kbps, None);
        }
        assert!(!update_download_speed_limit(&db, &queue, 999_999, 512).unwrap());
    }

    #[test]
    fn speed_limit_command_clamps_like_the_global_limit() {
        let db = Database::in_memory().unwrap();
        let queue = QueueManager::new();
        let id = insert_row(&db, DownloadStatus::Queued);
        assert!(update_download_speed_limit(&db, &queue, id, u32::MAX).unwrap());
        assert_eq!(db.get_download(id).unwrap().speed_limit_kbps, Some(1_048_576));
    }
}

#[cfg(test)]
mod control_core_tests {
    use super::{
        download_stats_for_state, pause_download_core, remove_download_core, resume_download_core,
        update_global_speed_limit_in_dir,
    };
    use crate::storage::models::{Download, DownloadCategory, DownloadStatus};
    use crate::util::lock_or_recover;
    use crate::AppState;

    fn insert_row(
        state: &AppState,
        filename: &str,
        status: DownloadStatus,
        save_path: &str,
    ) -> i64 {
        state
            .db
            .insert_download(&Download {
                id: None,
                url: format!("https://example.com/{filename}"),
                filename: filename.into(),
                save_path: save_path.into(),
                total_size: 100,
                downloaded_size: 25,
                status: status.clone(),
                category: DownloadCategory::Other,
                speed: 10.0,
                segments: 1,
                priority: 1,
                created_at: "t".into(),
                completed_at: None,
                error_message: (status == DownloadStatus::Failed).then(|| "HTTP 503".into()),
                referrer: None,
                user_agent: None,
                cookies: None,
                archived: false,
                attempts: 2,
                retry_at: Some(123),
                expected_hash: None,
                speed_limit_kbps: None,
                subtitle_tracks: None,
            })
            .unwrap()
    }

    #[tokio::test]
    async fn pause_core_pauses_queued_row_and_emits_once() {
        let state = crate::test_app_state("token", "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa");
        let id = insert_row(&state, "pause.bin", DownloadStatus::Queued, "/tmp");
        let mut emitted = Vec::new();
        pause_download_core(&state, id, |id| emitted.push(id)).await.unwrap();
        assert_eq!(emitted, vec![id]);
        let row = state.db.get_download(id).unwrap();
        assert_eq!(row.status, DownloadStatus::Paused);
        assert_eq!(row.speed, 0.0);
    }

    #[test]
    fn resume_core_requeues_failed_row_and_clears_retry_state() {
        let state = crate::test_app_state("token", "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa");
        let id = insert_row(&state, "resume.bin", DownloadStatus::Failed, "/tmp");
        let mut emitted = Vec::new();
        resume_download_core(&state, id, |id| emitted.push(id)).unwrap();
        assert_eq!(emitted, vec![id]);
        let row = state.db.get_download(id).unwrap();
        assert_eq!(row.status, DownloadStatus::Queued);
        assert_eq!(row.downloaded_size, 25, "partial progress is preserved");
        assert_eq!(row.attempts, 0);
        assert_eq!(row.retry_at, None);
        assert_eq!(row.error_message, None);
    }

    #[test]
    fn resume_core_rejects_completed_row() {
        let state = crate::test_app_state("token", "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa");
        let id = insert_row(&state, "done.bin", DownloadStatus::Completed, "/tmp");
        let err = resume_download_core(&state, id, |_| {}).unwrap_err();
        assert!(err.contains("already completed"));
    }

    #[tokio::test]
    async fn remove_core_deletes_completed_file_row_and_emits_once() {
        let dir = std::env::temp_dir().join(format!("falcon-remove-test-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let save_path = dir.to_string_lossy().to_string();
        std::fs::write(dir.join("done.bin"), b"finished").unwrap();
        let state = crate::test_app_state("token", "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa");
        let id = insert_row(&state, "done.bin", DownloadStatus::Completed, &save_path);
        let mut emitted = Vec::new();

        remove_download_core(&state, id, true, |id| emitted.push(id)).await.unwrap();

        assert_eq!(emitted, vec![id]);
        assert!(!dir.join("done.bin").exists());
        assert!(state.db.get_download(id).is_err());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn global_speed_limit_uses_settings_save_pipeline_and_keeps_memory() {
        let dir =
            std::env::temp_dir().join(format!("falcon-settings-test-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let state = crate::test_app_state("token", "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa");
        let mut settings = lock_or_recover(&state.settings).clone();
        settings.api_token = "token".into();
        settings.allowed_extension_ids.push("bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb".into());
        settings.schedule_active = true;
        settings.schedule_start = Some("23:00".into());
        settings.schedule_stop = Some("07:00".into());
        *lock_or_recover(&state.settings) = settings.clone();
        state.queue.set_schedule(crate::download::queue::ScheduleOptions {
            start_time: settings.schedule_start.clone(),
            stop_time: settings.schedule_stop.clone(),
            active: settings.schedule_active,
        });

        let applied = update_global_speed_limit_in_dir(&state, 512, &dir).unwrap();
        assert_eq!(applied, 512);
        assert_eq!(state.queue.speed_limit_kbps(), 512);
        assert_eq!(lock_or_recover(&state.settings).speed_limit_kbps, 512);
        assert_eq!(lock_or_recover(&state.settings).speed_limit_last_kbps, 512);
        assert_eq!(lock_or_recover(&state.api_token).as_str(), "token");
        assert_eq!(lock_or_recover(&state.settings).allowed_extension_ids.len(), 2);
        assert!(lock_or_recover(&state.settings).schedule_active);
        assert_eq!(lock_or_recover(&state.settings).schedule_start.as_deref(), Some("23:00"));
        assert_eq!(lock_or_recover(&state.settings).schedule_stop.as_deref(), Some("07:00"));
        assert!(dir.join("settings.json").is_file());

        let off = update_global_speed_limit_in_dir(&state, 0, &dir).unwrap();
        assert_eq!(off, 0);
        assert_eq!(state.queue.speed_limit_kbps(), 0);
        assert_eq!(lock_or_recover(&state.settings).speed_limit_last_kbps, 512);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn stats_core_counts_statuses_speed_bytes_and_live_limit() {
        let state = crate::test_app_state("token", "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa");
        insert_row(&state, "active.bin", DownloadStatus::Downloading, "/tmp");
        insert_row(&state, "queued.bin", DownloadStatus::Queued, "/tmp");
        insert_row(&state, "done.bin", DownloadStatus::Completed, "/tmp");
        let mut settings = lock_or_recover(&state.settings).clone();
        settings.speed_limit_kbps = 128;
        state.queue.apply_settings(&settings);

        let stats = download_stats_for_state(&state).unwrap();
        assert_eq!(stats.active, 1);
        assert_eq!(stats.queued, 1);
        assert_eq!(stats.completed, 1);
        assert_eq!(stats.all, 3);
        assert_eq!(stats.total_downloaded_bytes, 75);
        assert_eq!(stats.current_speed, 10.0);
        assert_eq!(stats.speed_limit_kbps, 128);
    }
}

#[cfg(test)]
mod refresh_tests {
    use super::{canonical_refresh_url, restore_state_file, rewrite_state_url};

    #[test]
    fn canonical_refresh_url_normalizes_case_port_and_transient_params() {
        assert_eq!(
            canonical_refresh_url("HTTPS://CDN.EXAMPLE.COM:443/a.bin").as_deref(),
            Ok("https://cdn.example.com/a.bin")
        );
        // Signed query bytes survive; only transient byte-range params are cut.
        assert_eq!(
            canonical_refresh_url(
                "https://rr1---sn.example.com/videoplayback?itag=18&sig=abc%2Bd%2F%3D&range=0-100&sq=1"
            )
            .as_deref(),
            Ok("https://rr1---sn.example.com/videoplayback?itag=18&sig=abc%2Bd%2F%3D")
        );
    }

    #[test]
    fn refresh_keeps_cookies_only_when_the_new_link_may_receive_them() {
        use super::refresh_keeps_cookies;
        let old = "https://cdn.example.com/file.bin?sig=old";
        // Same host, fresh signature: cookies still belong to this origin.
        assert!(refresh_keeps_cookies(old, "https://cdn.example.com/file.bin?sig=new"));
        assert!(refresh_keeps_cookies(old, "https://cdn.example.com/other/path.bin"));
        // A different host must never receive the old host's session cookies.
        assert!(!refresh_keeps_cookies(old, "https://evil.example.net/file.bin"));
        assert!(!refresh_keeps_cookies(old, "https://sub.cdn.example.com/file.bin"));
        // Scheme downgrade must not carry them either.
        assert!(!refresh_keeps_cookies(old, "http://cdn.example.com/file.bin"));
        // YouTube page cookies legitimately flow to googlevideo streams.
        assert!(refresh_keeps_cookies(
            "https://www.youtube.com/watch?v=abc",
            "https://rr1---sn-x.googlevideo.com/videoplayback?itag=18"
        ));
    }

    #[test]
    fn canonical_refresh_url_rejects_unsafe_and_junk_links() {
        assert_eq!(
            canonical_refresh_url("magnet:?xt=urn:btih:abc").unwrap_err(),
            "ERR_UNSUPPORTED_MAGNET"
        );
        assert_eq!(canonical_refresh_url("http://127.0.0.1/x").unwrap_err(), "invalid url");
        assert_eq!(canonical_refresh_url("ftp://example.com/x").unwrap_err(), "invalid url");
        assert_eq!(canonical_refresh_url("not a url").unwrap_err(), "invalid url");
        assert_eq!(
            canonical_refresh_url("https://www.youtube.com/generate_204").unwrap_err(),
            "not a real media url"
        );
    }

    fn state_fixture(dir: &std::path::Path) -> (String, String) {
        let save = dir.to_str().unwrap().to_string();
        (save, "file.bin".to_string())
    }

    fn write_state(dir: &std::path::Path, id: i64, json: &str) -> std::path::PathBuf {
        let (save, name) = state_fixture(dir);
        let path = crate::download::http::state_path_for(&save, &name, id).unwrap();
        std::fs::write(&path, json).unwrap();
        path
    }

    #[test]
    fn rewrite_state_url_swaps_only_the_url_and_returns_original_bytes() {
        let dir = std::env::temp_dir().join(format!("falcon-state-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let original = r#"{"version":1,"url":"https://cdn.example.com/expired","total":100,"validator":"\"etag-1\"","segments":[[0,99,40]],"future_field":true}"#;
        let path = write_state(&dir, 7, original);
        let (save, name) = state_fixture(&dir);

        let backup =
            rewrite_state_url(&save, &name, 7, "https://cdn.example.com/fresh").expect("rewritten");
        assert_eq!(backup, original.as_bytes(), "rollback needs the exact prior bytes");
        let rewritten: serde_json::Value =
            serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
        assert_eq!(rewritten["url"], "https://cdn.example.com/fresh");
        assert_eq!(rewritten["validator"], "\"etag-1\"");
        assert_eq!(rewritten["segments"], serde_json::json!([[0, 99, 40]]));
        assert_eq!(rewritten["future_field"], true, "unknown keys survive engine upgrades");

        // Rollback restores the file byte-exactly.
        restore_state_file(&save, &name, 7, &backup);
        assert_eq!(std::fs::read(&path).unwrap(), original.as_bytes());
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn rewrite_state_url_leaves_missing_foreign_and_synced_files_alone() {
        let dir = std::env::temp_dir().join(format!("falcon-state-none-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let (save, name) = state_fixture(&dir);

        // No state file at all: nothing to preserve, no error.
        assert!(rewrite_state_url(&save, &name, 3, "https://cdn.example.com/x").is_none());

        // Corrupt json is foreign to the engine too — untouched.
        let path = write_state(&dir, 3, "not json");
        assert!(rewrite_state_url(&save, &name, 3, "https://cdn.example.com/x").is_none());
        assert_eq!(std::fs::read(&path).unwrap(), b"not json");

        // Already in sync: no rewrite, no churn.
        write_state(
            &dir,
            3,
            r#"{"version":1,"url":"https://cdn.example.com/x","total":1,"segments":[[0,0,0]]}"#,
        );
        assert!(rewrite_state_url(&save, &name, 3, "https://cdn.example.com/x").is_none());
        let _ = std::fs::remove_dir_all(dir);
    }
}

#[cfg(test)]
mod clipboard_tests {
    use super::{normalize_pasteboard_text, read_clipboard_blocking};

    #[test]
    fn normalize_pasteboard_text_trims_and_caps_length() {
        assert_eq!(normalize_pasteboard_text("  hi  "), "hi");
        assert_eq!(normalize_pasteboard_text(&"x".repeat(3000)).len(), 2048);
    }

    /// Reads whatever is on the pasteboard. Never writes to it: a test must not
    /// replace what the developer copied.
    #[cfg(target_os = "macos")]
    #[test]
    fn read_clipboard_reads_pasteboard_without_modifying_it() {
        let got = read_clipboard_blocking().expect("pbpaste");
        assert!(got.chars().count() <= 2048);
        assert_eq!(got, got.trim());
    }
}

#[cfg(test)]
mod update_tests {
    use super::{github_release_page_url, version_newer};

    #[test]
    fn version_newer_compares_semver_tuples() {
        assert!(version_newer("0.2.0", "0.1.0"));
        assert!(!version_newer("0.1.0", "0.1.0"));
        assert!(!version_newer("0.1.0", "0.2.0"));
    }

    #[test]
    fn github_release_url_stays_on_this_repo() {
        assert_eq!(
            github_release_page_url("https://github.com/batu3384/falcon-dm/releases/tag/v0.2.0")
                .as_deref(),
            Some("https://github.com/batu3384/falcon-dm/releases/tag/v0.2.0")
        );
        assert!(github_release_page_url("https://evil.example/batu3384/falcon-dm").is_none());
        assert!(github_release_page_url("https://github.com/evil/falcon-dm/releases").is_none());
        assert!(github_release_page_url("https://github.com/batu3384/falcon-dm.evil/x").is_none());
        assert!(github_release_page_url("javascript:alert(1)").is_none());
        assert!(github_release_page_url("http://github.com/batu3384/falcon-dm/releases").is_none());
    }
}
