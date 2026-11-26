use crate::download::queue::{validate_schedule, ScheduleOptions, StopReason};
use crate::log_buffer;
use crate::pairing::PendingPair;
use crate::settings::Settings;
use crate::storage::models::{Download, DownloadFilter, DownloadStatus};
use crate::util::{
    app_data_dir, free_disk_space, lock_or_recover, move_file_exclusive, resolve_download_target,
    sanitize_filename, validate_open_path, LEGACY_DEFAULT_API_TOKEN,
};
use crate::{
    current_settings, emit_row, enqueue_download, parse_extension_id, remove_download_artifacts,
    resolve_download_save_path, AppState, ExternalDownloadPayload,
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
        },
    )
    .await?;
    state.db.get_download(id).map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn pause_download(
    app: AppHandle,
    id: i64,
    state: State<'_, AppState>,
) -> Result<(), String> {
    let row = state.db.get_download(id).map_err(|e| e.to_string())?;
    if row.status == DownloadStatus::Paused {
        return Ok(());
    }
    if state.queue.pause(&state.db, &app, id).await {
        Ok(())
    } else {
        Err("Download state changed before pause".into())
    }
}

#[tauri::command]
pub async fn resume_download(
    app: AppHandle,
    id: i64,
    state: State<'_, AppState>,
) -> Result<(), String> {
    let dl = state.db.get_download(id).map_err(|e| e.to_string())?;
    match dl.status {
        DownloadStatus::Completed => return Err("Download is already completed".into()),
        // Already on its way (or waiting out a retry backoff: start it now).
        DownloadStatus::Downloading | DownloadStatus::Merging => return Ok(()),
        DownloadStatus::Queued => {
            let _ = state.db.clear_retry_backoff(id);
            emit_row(&app, &state.db, id);
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
    emit_row(&app, &state.db, id);
    Ok(())
}

#[tauri::command]
pub async fn pause_all_downloads(app: AppHandle) -> Result<usize, String> {
    Ok(crate::pause_all(&app).await)
}

#[tauri::command]
pub async fn resume_all_downloads(app: AppHandle) -> Result<usize, String> {
    Ok(crate::resume_all(&app))
}

#[tauri::command]
pub async fn remove_download(
    app: AppHandle,
    id: i64,
    delete_file: Option<bool>,
    state: State<'_, AppState>,
) -> Result<(), String> {
    state.db.get_download(id).map_err(|e| e.to_string())?;
    if !state.queue.stop_and_wait(id, StopReason::Remove).await {
        return Err("Download is still stopping; try removing it again shortly".into());
    }
    // Read again: the worker may have renamed the file while finishing.
    let dl = state.db.get_download(id).map_err(|e| e.to_string())?;
    if delete_file.unwrap_or(false) && dl.status == DownloadStatus::Completed {
        let file_path = resolve_download_target(&dl.save_path, &dl.filename)?;
        if file_path.exists() {
            std::fs::remove_file(&file_path)
                .map_err(|e| format!("Could not delete file from disk: {e}"))?;
        }
    }
    remove_download_artifacts(&dl.save_path, &dl.filename, id);
    state.db.delete_download(id).map_err(|e| e.to_string())?;
    let _ = app.emit("download-removed", id);
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
    if let Some(search) = filter.search.take() {
        let clipped: String = search.chars().take(200).collect();
        filter.search = if clipped.trim().is_empty() { None } else { Some(clipped) };
    }
    if let Some(ref mut statuses) = filter.statuses {
        statuses.truncate(8);
    }
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

#[tauri::command]
pub async fn open_folder(path: String) -> Result<(), String> {
    let safe = validate_open_path(&path)?;
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
pub async fn open_file(path: String) -> Result<(), String> {
    let safe = validate_open_path(&path)?;
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
    lock_or_recover(&state.pairing).resolve(id);
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
/// extensions that stay approved pick up the new one on their next request.
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

/// Which external tools the engines will use. Shown in Settings and onboarding.
#[derive(serde::Serialize)]
pub(crate) struct ToolStatus {
    ytdlp_path: Option<String>,
    ytdlp_version: Option<String>,
    /// Error code when yt-dlp is unusable (`ERR_YTDLP_MISSING`, `ERR_YTDLP_PATH_INVALID: …`).
    ytdlp_error: Option<String>,
    ffmpeg_path: Option<String>,
}

#[tauri::command]
pub async fn get_tool_status(state: State<'_, AppState>) -> Result<ToolStatus, String> {
    use crate::download::tools::{augmented_path, find_ffmpeg, find_ytdlp};
    let preferred = lock_or_recover(&state.settings).ytdlp_path.clone();
    let (ytdlp, ffmpeg) =
        tokio::task::spawn_blocking(move || (find_ytdlp(Some(&preferred)), find_ffmpeg()))
            .await
            .map_err(|e| e.to_string())?;
    let mut status = ToolStatus {
        ytdlp_path: None,
        ytdlp_version: None,
        ytdlp_error: None,
        ffmpeg_path: ffmpeg.ok().map(|p| p.to_string_lossy().to_string()),
    };
    match ytdlp {
        Ok(path) => {
            let version = tokio::time::timeout(
                std::time::Duration::from_secs(8),
                tokio::process::Command::new(&path)
                    .arg("--version")
                    .env("PATH", augmented_path())
                    .stdin(std::process::Stdio::null())
                    .kill_on_drop(true)
                    .output(),
            )
            .await;
            if let Ok(Ok(output)) = version {
                if output.status.success() {
                    let text = String::from_utf8_lossy(&output.stdout).trim().to_string();
                    status.ytdlp_version =
                        (!text.is_empty()).then(|| text.chars().take(40).collect());
                }
            }
            status.ytdlp_path = Some(path.to_string_lossy().to_string());
        }
        Err(error) => status.ytdlp_error = Some(error),
    }
    Ok(status)
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

#[tauri::command]
pub async fn get_stats(state: State<'_, AppState>) -> Result<DownloadStats, String> {
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

#[tauri::command]
pub async fn save_settings(settings: Settings, state: State<'_, AppState>) -> Result<(), String> {
    let current = current_settings(&state);
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
    settings.save(&app_data_dir())?;

    state.queue.apply_settings(&settings);
    *lock_or_recover(&state.api_token) = settings.api_token.clone();
    *lock_or_recover(&state.settings) = settings;
    Ok(())
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
