use crate::download::queue::{validate_schedule, QueueManager, ScheduleOptions, StopReason};
use crate::log_buffer;
use crate::pairing::PairingState;
use crate::settings::Settings;
use crate::storage::models::{DownloadFilter, DownloadStatus};
use crate::storage::Database;
use crate::util::{self, app_data_dir};
use crate::{handle_deep_link_url, AppState};
use axum::{
    http::{header, HeaderName, Method},
    middleware::from_fn_with_state,
    routing::{get, post},
    Router,
};
use futures::FutureExt;
use std::collections::{HashSet, VecDeque};
use std::panic::AssertUnwindSafe;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use tauri::{
    menu::{Menu, MenuItem},
    tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent},
    AppHandle, Manager,
};
use window_vibrancy::{apply_vibrancy, NSVisualEffectMaterial};

/// Rows that were live when the app last exited go back to the queue.
fn restore_orphan_downloads(db: &Database) {
    let live = [DownloadStatus::Downloading, DownloadStatus::Merging];
    if let Ok(rows) =
        db.get_downloads(&DownloadFilter { statuses: Some(live.to_vec()), ..Default::default() })
    {
        for id in rows.into_iter().filter_map(|row| row.id) {
            if let Err(e) =
                db.set_status_error_if_current(id, &live, &DownloadStatus::Queued, None, Some(0.0))
            {
                log::warn!("restore: download {id} could not be requeued: {e}");
            }
        }
    }
    let done = [DownloadStatus::Completed, DownloadStatus::Failed];
    if let Ok(rows) =
        db.get_downloads(&DownloadFilter { statuses: Some(done.to_vec()), ..Default::default() })
    {
        for id in rows.into_iter().filter_map(|row| row.id) {
            let _ = db.clear_session_cookies(id);
        }
    }
}

/// Ids whose partial data must survive a restart (anything not finished).
fn resumable_ids(db: &Database) -> HashSet<i64> {
    let statuses = vec![
        DownloadStatus::Queued,
        DownloadStatus::Downloading,
        DownloadStatus::Merging,
        DownloadStatus::Paused,
        DownloadStatus::Failed,
    ];
    let mut ids = HashSet::new();
    for archived in [None, Some(true)] {
        if let Ok(rows) = db.get_downloads(&DownloadFilter {
            statuses: Some(statuses.clone()),
            archived,
            ..Default::default()
        }) {
            ids.extend(rows.into_iter().filter_map(|row| row.id));
        }
    }
    ids
}

/// `hls-<id>.falcondm-temp` → id.
fn hls_dir_id(name: &str) -> Option<i64> {
    name.strip_prefix("hls-")?.strip_suffix(".falcondm-temp")?.parse().ok()
}

/// `.<name>.<id>.falcon.part|state` → id.
fn http_temp_id(name: &str) -> Option<i64> {
    let stem = name
        .strip_suffix(".falcon.part")
        .or_else(|| name.strip_suffix(".falcon.state"))
        .or_else(|| name.strip_suffix(".falcon.segments"))?;
    if !name.starts_with('.') {
        return None;
    }
    stem.rsplit('.').next()?.parse().ok()
}

/// Delete HLS segment caches that no unfinished download owns.
fn cleanup_stale_hls_dirs(root: &std::path::Path, keep: &HashSet<i64>) {
    let Ok(entries) = std::fs::read_dir(root.join("downloads_temp")) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        let name = entry.file_name().to_string_lossy().to_string();
        let stale = match hls_dir_id(&name) {
            Some(id) => !keep.contains(&id),
            // Legacy layouts: `hls-<id>` and `<anything>.falcondm-temp`.
            None => name.ends_with(".falcondm-temp") || name.starts_with("hls-"),
        };
        if stale && path.is_dir() {
            let _ = std::fs::remove_dir_all(&path);
        }
    }
}

/// Delete orphaned partial files and yt-dlp work dirs under the download
/// folders. Only names this app generates are touched.
fn cleanup_stale_download_temps(roots: &[std::path::PathBuf], keep: &HashSet<i64>) {
    let mut pending: Vec<(std::path::PathBuf, usize)> =
        roots.iter().map(|root| (root.clone(), 0)).collect();
    let mut seen = HashSet::new();
    while let Some((dir, depth)) = pending.pop() {
        if !seen.insert(dir.clone()) {
            continue;
        }
        let Ok(entries) = std::fs::read_dir(&dir) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            let name = entry.file_name().to_string_lossy().to_string();
            let is_dir = entry.file_type().map(|kind| kind.is_dir()).unwrap_or(false);
            if let Some(id) = crate::download::ytdlp::parse_run_dir_name(&name) {
                // Old builds used `-<uuid>` suffixed dirs that nothing can resume.
                let current = name == format!(".falcon-dm-ytdlp-{id}");
                if is_dir && (!current || !keep.contains(&id)) {
                    let _ = std::fs::remove_dir_all(&path);
                }
            } else if let Some(id) = http_temp_id(&name) {
                if !keep.contains(&id) || name.ends_with(".falcon.segments") {
                    if is_dir {
                        let _ = std::fs::remove_dir_all(&path);
                    } else {
                        let _ = std::fs::remove_file(&path);
                    }
                }
            } else if is_dir && depth < 2 && !name.starts_with('.') {
                // Bounded walk: category and profile subfolders, not the whole tree.
                pending.push((path, depth + 1));
            }
        }
    }
}

fn download_roots(settings: &Settings) -> Vec<std::path::PathBuf> {
    let mut roots = vec![util::default_download_dir()];
    let custom = settings.default_download_path.trim();
    if !custom.is_empty() {
        roots.push(util::expand_tilde(custom));
    }
    roots.extend(
        settings
            .category_paths
            .values()
            .filter(|p| !p.trim().is_empty())
            .map(|p| util::expand_tilde(p)),
    );
    roots.retain(|root| !util::is_unsafe_download_root(root));
    roots
}

fn spawn_local_api(app_handle: tauri::AppHandle) {
    tauri::async_runtime::spawn(async move {
        use tauri::http::HeaderValue;
        use tower_http::cors::{AllowOrigin, CorsLayer};

        let cors = CorsLayer::new()
            .allow_origin(AllowOrigin::predicate(|origin: &HeaderValue, _| {
                origin.to_str().is_ok_and(|origin| {
                    origin.starts_with("chrome-extension://")
                        || origin.starts_with("edge-extension://")
                })
            }))
            // Health is GET; browsers may OPTIONS-preflight localhost (private network).
            .allow_methods([Method::GET, Method::POST, Method::OPTIONS])
            .allow_headers([header::CONTENT_TYPE, HeaderName::from_static("x-falcon-token")])
            .allow_private_network(true);

        let app = Router::new()
            .route("/api/health", get(crate::local_api::handle_health))
            .route("/api/pair", post(crate::local_api::handle_pair))
            .route("/api/ping", post(crate::local_api::handle_ping))
            .route("/api/downloads", get(crate::local_api::handle_downloads))
            .route("/api/add", post(crate::local_api::handle_api_add))
            .route("/api/intercept", post(crate::local_api::handle_intercept))
            .route("/api/upload", post(crate::local_api::handle_api_upload))
            // Blob uploads are capped at 32 MB (≈43 MB as base64 JSON).
            .layer(axum::extract::DefaultBodyLimit::max(48 * 1024 * 1024))
            .layer(from_fn_with_state(app_handle.clone(), crate::local_api::rate_limit_middleware))
            .layer(cors)
            .with_state(app_handle);

        match tokio::net::TcpListener::bind("127.0.0.1:14201").await {
            Ok(listener) => {
                log::info!("Local API listening on 127.0.0.1:14201");
                if let Err(e) = axum::serve(listener, app).await {
                    log::error!("Local API server error: {e}");
                }
            }
            Err(e) => {
                log::error!(
                    "Could not bind 127.0.0.1:14201 ({e}); the browser extension cannot connect"
                );
            }
        }
    });
}

fn spawn_queue_loop(app_handle: tauri::AppHandle) {
    tauri::async_runtime::spawn(async move {
        let mut interval = tokio::time::interval(std::time::Duration::from_millis(500));
        interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        loop {
            interval.tick().await;
            // One panic must not freeze download management.
            let tick = AssertUnwindSafe(async {
                let state = app_handle.state::<AppState>();
                state.queue.tick(&state.db, &app_handle)
            })
            .catch_unwind()
            .await;
            match tick {
                Ok(Ok(())) => {}
                Ok(Err(e)) => log::warn!("queue tick failed: {e}"),
                Err(panic) => log::error!("queue tick panicked, continuing: {panic:?}"),
            }
        }
    });
}

const TRAY_ID: &str = "main";

fn locale_is_turkish(lang: &str) -> bool {
    lang.trim().split(['-', '_', '.']).next().is_some_and(|tag| tag.eq_ignore_ascii_case("tr"))
}

fn tray_menu(app: &AppHandle, lang: &str) -> tauri::Result<Menu<tauri::Wry>> {
    let tr = locale_is_turkish(lang);
    let item = |id: &str, en: &str, turkish: &str| {
        MenuItem::with_id(app, id, if tr { turkish } else { en }, true, None::<&str>)
    };
    let show_i = item("show", "Show Falcon DM", "Falcon DM'i Göster")?;
    let pause_i = item("pause", "Pause All", "Tümünü Duraklat")?;
    let resume_i = item("resume", "Resume All", "Tümünü Sürdür")?;
    let quit_i = item("quit", "Quit", "Çıkış")?;
    Menu::with_items(app, &[&show_i, &pause_i, &resume_i, &quit_i])
}

fn setup_tray(app: &tauri::App, lang: &str) -> tauri::Result<()> {
    let handle = app.handle().clone();
    let menu = tray_menu(&handle, lang)?;
    let mut builder = TrayIconBuilder::with_id(TRAY_ID)
        .menu(&menu)
        .show_menu_on_left_click(false)
        .on_menu_event(|app, event| match event.id.as_ref() {
            "quit" => app.exit(0),
            "show" => crate::show_main_window(app),
            "pause" => {
                let app = app.clone();
                tauri::async_runtime::spawn(async move {
                    crate::pause_all(&app).await;
                });
            }
            "resume" => {
                crate::resume_all(app);
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
                crate::show_main_window(tray.app_handle());
            }
        });
    if let Some(icon) = app.default_window_icon() {
        builder = builder.icon(icon.clone());
    }
    builder.build(app)?;
    Ok(())
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    // Records go to stderr (RUST_LOG-aware) and into the ring buffer behind the Logs panel.
    log_buffer::install();

    let data_dir = app_data_dir();
    let _ = std::fs::create_dir_all(&data_dir);
    let db = Database::init(&data_dir).expect("Failed to init DB");
    restore_orphan_downloads(&db);

    let settings = Settings::load(&data_dir);
    // Partial data of unfinished downloads is what makes resume possible: only
    // temp files nobody owns any more are removed.
    let keep = resumable_ids(&db);
    cleanup_stale_hls_dirs(&data_dir, &keep);
    cleanup_stale_download_temps(&download_roots(&settings), &keep);

    let queue = QueueManager::new();
    queue.apply_settings(&settings);
    let initial_schedule = ScheduleOptions {
        start_time: settings.schedule_start.clone(),
        stop_time: settings.schedule_stop.clone(),
        active: settings.schedule_active,
    };
    if validate_schedule(&initial_schedule).is_ok() {
        queue.set_schedule(initial_schedule);
    } else {
        log::warn!("Ignoring invalid persisted scheduler configuration");
    }

    let system_lang = std::env::var("LANG").unwrap_or_default();
    let app_state = AppState {
        db,
        queue,
        api_token: Arc::new(Mutex::new(settings.api_token.clone())),
        settings: Arc::new(Mutex::new(settings)),
        rate_bucket: Arc::new(Mutex::new(VecDeque::new())),
        pairing: Arc::new(Mutex::new(PairingState::default())),
        ui_lang: Arc::new(Mutex::new(system_lang.clone())),
    };

    #[cfg(desktop)]
    let builder = tauri::Builder::default()
        .plugin(tauri_plugin_single_instance::init(|app, _argv, _cwd| {
            crate::show_main_window(app);
        }))
        .plugin(tauri_plugin_window_state::Builder::default().build());
    #[cfg(not(desktop))]
    let builder = tauri::Builder::default();

    let app = builder
        .plugin(tauri_plugin_deep_link::init())
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_notification::init())
        .plugin(tauri_plugin_dialog::init())
        .setup(move |app| {
            let app_handle = app.handle().clone();

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
                let _ = apply_vibrancy(&window, NSVisualEffectMaterial::Sidebar, None, None);
            }

            setup_tray(app, &system_lang)?;
            spawn_local_api(app_handle.clone());
            spawn_queue_loop(app_handle);
            Ok(())
        })
        .manage(app_state)
        .invoke_handler(tauri::generate_handler![
            crate::commands::add_download,
            crate::commands::pause_download,
            crate::commands::resume_download,
            crate::commands::pause_all_downloads,
            crate::commands::resume_all_downloads,
            crate::commands::remove_download,
            crate::commands::get_downloads,
            crate::commands::get_download,
            crate::commands::set_schedule,
            crate::commands::get_schedule,
            crate::commands::change_priority,
            crate::commands::get_settings,
            crate::commands::save_settings,
            crate::commands::get_extension_status,
            crate::commands::approve_extension_pair,
            crate::commands::reject_extension_pair,
            crate::commands::revoke_extension,
            crate::commands::get_tool_status,
            crate::commands::get_logs,
            crate::commands::clear_logs,
            crate::commands::get_stats,
            crate::commands::move_download,
            crate::commands::archive_download,
            crate::commands::open_folder,
            crate::commands::open_file,
            crate::commands::check_for_updates,
            crate::commands::read_clipboard,
            set_ui_language
        ])
        .build(tauri::generate_context!())
        .expect("error while building tauri application");

    app.run(|app_handle, event| {
        match event {
            tauri::RunEvent::ExitRequested { api, code, .. } => {
                // Restart ignores prevent_exit. A second ExitRequested is the real exit.
                if code == Some(tauri::RESTART_EXIT_CODE) {
                    return;
                }
                if start_shutdown(app_handle) {
                    api.prevent_exit();
                }
            }
            tauri::RunEvent::WindowEvent {
                label,
                event: tauri::WindowEvent::CloseRequested { api, .. },
                ..
            } if label == "main" => {
                api.prevent_close();
                if let Some(window) = app_handle.get_webview_window("main") {
                    let _ = window.hide();
                }
            }
            #[cfg(target_os = "macos")]
            tauri::RunEvent::Reopen { has_visible_windows, .. } if !has_visible_windows => {
                crate::show_main_window(app_handle);
            }
            _ => {}
        }
    });
}

/// Stop every worker and give them a moment to persist resume state.
fn start_shutdown(app_handle: &AppHandle) -> bool {
    static STARTED: AtomicBool = AtomicBool::new(false);
    if STARTED.swap(true, Ordering::SeqCst) {
        return false;
    }
    let app_handle = app_handle.clone();
    tauri::async_runtime::spawn(async move {
        let state = app_handle.state::<AppState>();
        let ids = state.queue.active_ids();
        for id in &ids {
            state.queue.request_stop(*id, StopReason::Shutdown);
        }
        let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(3);
        while tokio::time::Instant::now() < deadline
            && ids.iter().any(|id| state.queue.is_active(*id))
        {
            tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        }
        log::info!("Falcon DM shutting down ({} active downloads stopped)", ids.len());
        app_handle.exit(0);
    });
    true
}

#[tauri::command]
pub fn set_ui_language(app: AppHandle, lang: String) -> Result<(), String> {
    *util::lock_or_recover(&app.state::<AppState>().ui_lang) = lang.clone();
    let menu = tray_menu(&app, &lang).map_err(|e| e.to_string())?;
    app.tray_by_id(TRAY_ID)
        .ok_or_else(|| "tray icon missing".to_string())?
        .set_menu(Some(menu))
        .map_err(|e| e.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn locale_is_turkish_matches_tr_tags() {
        assert!(locale_is_turkish("tr"));
        assert!(locale_is_turkish("tr-TR"));
        assert!(locale_is_turkish("tr_TR.UTF-8"));
        assert!(!locale_is_turkish("en"));
        assert!(!locale_is_turkish("en_US"));
    }

    #[test]
    fn temp_names_map_back_to_download_ids() {
        assert_eq!(hls_dir_id("hls-42.falcondm-temp"), Some(42));
        assert_eq!(hls_dir_id("hls-42"), None);
        assert_eq!(http_temp_id(".movie.mp4.7.falcon.part"), Some(7));
        assert_eq!(http_temp_id(".a.b.c.123.falcon.state"), Some(123));
        assert_eq!(http_temp_id("movie.mp4.7.falcon.part"), None, "must be a hidden file");
        assert_eq!(http_temp_id(".movie.mp4.falcon.part"), None);
        assert_eq!(http_temp_id("notes.part"), None);
    }

    #[test]
    fn startup_cleanup_keeps_resumable_data_and_user_files() {
        let root = std::env::temp_dir().join(format!("falcon-cleanup-{}", uuid::Uuid::new_v4()));
        let sub = root.join("Videos");
        std::fs::create_dir_all(&sub).unwrap();
        let keep: HashSet<i64> = [7].into_iter().collect();

        let kept_part = root.join(".big.iso.7.falcon.part");
        let kept_state = root.join(".big.iso.7.falcon.state");
        let stale_part = sub.join(".old.zip.3.falcon.part");
        let user_file = root.join("holiday.part");
        let user_hidden = root.join(".DS_Store");
        for file in [&kept_part, &kept_state, &stale_part, &user_file, &user_hidden] {
            std::fs::write(file, b"x").unwrap();
        }
        let kept_run = root.join(".falcon-dm-ytdlp-7");
        let stale_run = root.join(".falcon-dm-ytdlp-3");
        let legacy_run = root.join(".falcon-dm-ytdlp-7-123e4567-e89b-12d3-a456-426614174000");
        for dir in [&kept_run, &stale_run, &legacy_run] {
            std::fs::create_dir_all(dir).unwrap();
        }

        cleanup_stale_download_temps(std::slice::from_ref(&root), &keep);
        assert!(kept_part.exists() && kept_state.exists() && kept_run.exists());
        assert!(!stale_part.exists() && !stale_run.exists() && !legacy_run.exists());
        assert!(user_file.exists() && user_hidden.exists(), "foreign files are never touched");

        let data = root.join("data");
        let temp = data.join("downloads_temp");
        std::fs::create_dir_all(temp.join("hls-7.falcondm-temp")).unwrap();
        std::fs::create_dir_all(temp.join("hls-3.falcondm-temp")).unwrap();
        cleanup_stale_hls_dirs(&data, &keep);
        assert!(temp.join("hls-7.falcondm-temp").exists());
        assert!(!temp.join("hls-3.falcondm-temp").exists());
        std::fs::remove_dir_all(root).unwrap();
    }
}
