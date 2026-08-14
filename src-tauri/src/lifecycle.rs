use crate::download::engine::Aria2Engine;
use crate::download::queue::{validate_schedule, QueueManager, ScheduleOptions};
use crate::log_buffer;
use crate::native_messaging::{self, PairProofStore};
use crate::settings::Settings;
use crate::storage::models::{DownloadFilter, DownloadStatus};
use crate::storage::Database;
use crate::util::{self, app_data_dir, is_hls_url};
use crate::{fail_invalid_download, finalize_completed_download, handle_deep_link_url, AppState};
use axum::{
    http::{header, HeaderName, Method},
    middleware::from_fn_with_state,
    routing::{get, post},
    Router,
};
use futures::FutureExt;
use serde::Serialize;
use std::collections::{HashMap, VecDeque};
use std::panic::AssertUnwindSafe;
use std::sync::{Arc, Mutex};
use tauri::{
    menu::{Menu, MenuItem},
    tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent},
    Emitter, Manager,
};
use tauri_plugin_notification::NotificationExt;
use window_vibrancy::{apply_vibrancy, NSVisualEffectMaterial};

#[derive(Clone, Serialize)]
struct ProgressPayload {
    id: i64,
    downloaded_size: u64,
    total_size: u64,
    speed: f64,
    status: String,
    connections: u32,
}

fn restore_orphan_downloads(db: &Database) {
    for status in [DownloadStatus::Downloading, DownloadStatus::Merging] {
        let Ok(active) = db.get_downloads(&DownloadFilter {
            status: Some(status.clone()),
            limit: Some(10_000),
            ..Default::default()
        }) else {
            continue;
        };
        for dl in active {
            let Some(id) = dl.id else {
                continue;
            };
            if let Err(e) = db.clear_aria2_gid_if_current(id, std::slice::from_ref(&status)) {
                log::warn!("restore: orphan {id} gid clear failed: {e}");
            }
            if let Err(e) = db.set_status_error_if_current(
                id,
                std::slice::from_ref(&status),
                &DownloadStatus::Queued,
                None,
                Some(0.0),
            ) {
                log::warn!("restore: orphan {id} status update failed: {e}");
            }
        }
    }
    for status in [DownloadStatus::Completed, DownloadStatus::Failed] {
        if let Ok(done) = db.get_downloads(&DownloadFilter {
            status: Some(status),
            limit: Some(10_000),
            ..Default::default()
        }) {
            for dl in done {
                if let Some(id) = dl.id {
                    if let Err(e) = db.clear_session_cookies(id) {
                        log::warn!("restore: cookie wipe {id} DB update failed: {e}");
                    }
                }
            }
        }
    }
}

fn cleanup_stale_http_parts(db: &Database) {
    let Ok(completed) = db.get_downloads(&DownloadFilter {
        status: Some(DownloadStatus::Completed),
        archived: None,
        limit: Some(10_000),
        ..Default::default()
    }) else {
        return;
    };
    let Ok(archived) = db.get_downloads(&DownloadFilter {
        status: Some(DownloadStatus::Completed),
        archived: Some(true),
        limit: Some(10_000),
        ..Default::default()
    }) else {
        return;
    };
    for download in completed.into_iter().chain(archived) {
        let Some(id) = download.id else {
            continue;
        };
        if let Some(temp) =
            crate::download::http::part_path_for(&download.save_path, &download.filename, id)
        {
            let _ = std::fs::remove_file(temp);
        }
    }
}

/// Remove leftover `*.falcondm-temp` dirs (crash recovery) under `root`.
/// ponytail: HLS temp dirs now live under `<data_dir>/downloads_temp/`, so this
/// sweep actually finds crash leftovers (previously it scanned the data_dir root
/// while HLS wrote temp dirs under the Downloads folder → never matched).
fn cleanup_stale_temp_dirs(root: &std::path::Path) {
    let temp_root = root.join("downloads_temp");
    let Ok(entries) = std::fs::read_dir(&temp_root) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            if let Some(name) = path.file_name().and_then(|n| n.to_str()) {
                if name.ends_with(".falcondm-temp") {
                    let _ = std::fs::remove_dir_all(&path);
                }
            }
        }
    }
}

fn cleanup_stale_ytdlp_dirs(root: &std::path::Path) {
    let mut candidates = vec![(root.to_path_buf(), 0usize)];
    while let Some((parent, depth)) = candidates.pop() {
        let Ok(entries) = std::fs::read_dir(parent) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if !entry.file_type().map(|kind| kind.is_dir()).unwrap_or(false) {
                continue;
            }
            let is_stale = path.file_name().and_then(|name| name.to_str()).is_some_and(|name| {
                name.strip_prefix(".falcon-dm-ytdlp-")
                    .and_then(|suffix| suffix.split_once('-'))
                    .and_then(|(id, run)| {
                        id.parse::<i64>().ok().and_then(|_| uuid::Uuid::parse_str(run).ok())
                    })
                    .is_some()
            });
            if is_stale {
                let _ = std::fs::remove_dir_all(path);
            } else if depth < 8 {
                // ponytail: bound startup filesystem work; deeper custom roots
                // need explicit cleanup rather than an unbounded recursive walk.
                candidates.push((path, depth + 1));
            }
        }
    }
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    // ponytail: install the fan-out logger: records go to stderr (RUST_LOG-aware,
    // env_logger formatted) AND into the in-memory ring buffer that the frontend
    // "Logs" panel reads via get_logs. Replaces the bare env_logger init.
    log_buffer::install();

    let data_dir = app_data_dir();
    let _ = std::fs::create_dir_all(&data_dir);
    cleanup_stale_temp_dirs(&data_dir);
    let db = Database::init(&data_dir).expect("Failed to init DB");
    restore_orphan_downloads(&db);
    cleanup_stale_http_parts(&db);

    let settings = Settings::load(&data_dir);
    let download_root = if settings.default_download_path.trim().is_empty() {
        util::default_download_dir()
    } else {
        util::expand_tilde(&settings.default_download_path)
    };
    cleanup_stale_ytdlp_dirs(&download_root);
    let engine = Aria2Engine::new();
    let queue = QueueManager::new();
    queue.set_concurrent_downloads(settings.max_concurrent_downloads as usize);
    queue.set_max_connections(settings.max_connections_per_server as usize);
    queue.set_http_options(settings.proxy.clone(), settings.speed_limit_kbps);
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

    let app_state = AppState {
        db,
        engine,
        queue,
        api_token: Arc::new(Mutex::new(settings.api_token.clone())),
        settings: Arc::new(Mutex::new(settings.clone())),
        rate_bucket: Arc::new(Mutex::new(VecDeque::new())),
        pending_pair_ids: Arc::new(Mutex::new(VecDeque::new())),
        pair_proofs: Arc::new(PairProofStore::default()),
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

            if let Err(error) =
                native_messaging::start_pairing_server(&data_dir, state.pair_proofs.clone())
            {
                log::error!("failed to start native pairing server: {error}");
            }

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

            log::info!("aria2 stays idle until a legacy GID needs recovery");

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
                            let mut downloads = state.db.get_downloads(&DownloadFilter {
                                status: Some(DownloadStatus::Downloading),
                                limit: Some(256),
                                ..Default::default()
                            });
                            if let Ok(mut merging) = state.db.get_downloads(&DownloadFilter {
                                status: Some(DownloadStatus::Merging),
                                limit: Some(256),
                                ..Default::default()
                            }) {
                                if let Ok(ref mut active) = downloads {
                                    active.append(&mut merging);
                                }
                            }
                            if let Ok(downloads) = downloads {
                                for dl in downloads {
                                    if let Some(gid) = &dl.aria2_gid {
                                        let _ = state.engine.pause(gid).await;
                                    }
                                    if let Some(id) = dl.id {
                                        if !state.queue.cancel_and_wait_stream(id).await {
                                            log::warn!("tray pause timed out for download {id}");
                                            continue;
                                        }
                                    }
                                    let id = dl.id.unwrap();
                                    match state.db.set_status_error_if_current(
                                        id,
                                        &[DownloadStatus::Downloading, DownloadStatus::Merging],
                                        &DownloadStatus::Paused,
                                        None,
                                        Some(0.0),
                                    ) {
                                        Ok(true) => {}
                                        Ok(false) => {}
                                        Err(e) => {
                                            log::warn!("tray pause-all: {id} DB update failed: {e}");
                                        }
                                    }
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
                                for dl in downloads {
                                    // Respect queue concurrency — mark Queued, let tick start
                                    let id = dl.id.unwrap();
                                    if let Err(e) = state
                                        .db
                                        .set_status_if_current(
                                            id,
                                            &[DownloadStatus::Paused],
                                            &DownloadStatus::Queued,
                                        )
                                    {
                                        log::warn!("tray resume-all: {id} DB update failed: {e}");
                                    }
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
                    // ponytail: least-privilege — the API only accepts POSTs and the
                    // two headers the extension actually sends (JSON content type + our
                    // auth token). Any-method/Any-header widened the attack surface.
                    .allow_methods([Method::POST])
                    .allow_headers([
                        header::CONTENT_TYPE,
                        HeaderName::from_static("x-falcon-token"),
                    ])
                    .allow_private_network(true);

                let app = Router::new()
                    .route("/api/health", get(crate::local_api::handle_health))
                    .route("/api/pair", post(crate::local_api::handle_pair))
                    .route("/api/ping", post(crate::local_api::handle_ping))
                    .route("/api/add", post(crate::local_api::handle_api_add))
                    .route("/api/intercept", post(crate::local_api::handle_intercept))
                    .layer(from_fn_with_state(
                        axum_app_handle.clone(),
                        crate::local_api::rate_limit_middleware,
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
                    // ponytail: panic-guard each loop body — one panic must not freeze download management.
                    if let Err(p) = AssertUnwindSafe(async {
                        let st = app_handle.state::<AppState>();
                        st.queue
                            .tick(&st.db, &st.engine, app_handle.clone())
                            .await
                    })
                    .catch_unwind()
                    .await
                    {
                        log::error!("queue tick panicked, continuing: {:?}", p);
                    }

                    if let Err(p) = AssertUnwindSafe(async {
                        let state = app_handle.state::<AppState>();
                    if let Ok(downloads) = state.db.get_downloads(&DownloadFilter {
                        status: Some(DownloadStatus::Downloading),
                        limit: Some(256),
                        ..Default::default()
                    }) {
                        // Batch all active aria2 statuses in one RPC; fall back to per-gid tellStatus
                        // only for gids not present (complete/error/paused don't appear in tellActive).
                        let active_map: HashMap<String, serde_json::Value> =
                            if state.engine.is_running() {
                            match state.engine.get_active_statuses().await {
                                Ok(arr) => arr
                                    .into_iter()
                                    .filter_map(|s| {
                                        // extract gid as owned String first so we can move `s` next
                                        let gid = s
                                            .get("gid")
                                            .and_then(|g| g.as_str())
                                            .map(ToString::to_string)?;
                                        Some((gid, s))
                                    })
                                    .collect(),
                                Err(e) => {
                                    let msg = e.to_string().to_lowercase();
                                    if !(msg.contains("not running")
                                        || msg.contains("network")
                                        || msg.contains("timed out")
                                        || msg.contains("connection"))
                                    {
                                        log::warn!("tellActive failed: {}", e);
                                    }
                                    HashMap::new()
                                }
                            }
                            } else {
                                HashMap::new()
                            };
                        for mut dl in downloads {
                            let mut speed = 0.0;
                            let mut status_str = "Downloading".to_string();
                            let mut connections = 8u32;

                            if let Some(gid) = &dl.aria2_gid {
                                let status_result = if let Some(s) = active_map.get(gid) {
                                    Ok(s.clone())
                                } else {
                                    state.engine.get_status(gid).await
                                };
                                match status_result {
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

                            let poll_id = dl.id.unwrap();
                            let committed = match state.db.update_progress_if_current(
                                poll_id,
                                &[DownloadStatus::Downloading],
                                dl.downloaded_size,
                                dl.total_size.max(dl.downloaded_size),
                                speed,
                                &dl.status,
                                dl.completed_at.as_deref(),
                                dl.error_message.as_deref(),
                            ) {
                                Ok(value) => value,
                                Err(e) => {
                                    log::warn!("progress poll: {poll_id} DB update failed: {e}");
                                    false
                                }
                            };
                            if !committed {
                                continue;
                            }

                            if matches!(status_str.as_str(), "Completed" | "Failed") {
                                let _ = state.db.clear_session_cookies(poll_id);
                            }

                            if status_str == "Completed" {
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
                                // ponytail: guard stale/underreported total so the UI never shows >100%.
                                total_size: dl.total_size.max(dl.downloaded_size),
                                speed,
                                status: status_str,
                                connections,
                            };
                            let _ = app_handle.emit("download-progress", payload);
                        }
                    }
                    })
                    .catch_unwind()
                    .await
                    {
                        log::error!("progress poll panicked, continuing: {:?}", p);
                    }
                }
            });
            Ok(())
        })
        .manage(app_state)
        .invoke_handler(tauri::generate_handler![
            crate::commands::add_download,
            crate::commands::pause_download,
            crate::commands::resume_download,
            crate::commands::remove_download,
            crate::commands::get_downloads,
            crate::commands::get_download,
            crate::commands::get_download_status,
            crate::commands::set_schedule,
            crate::commands::get_schedule,
            crate::commands::change_priority,
            crate::commands::get_settings,
            crate::commands::save_settings,
            crate::commands::get_extension_status,
            crate::commands::reset_extension_pin,
            crate::commands::get_pending_pair,
            crate::commands::get_pending_pairs,
            crate::commands::approve_extension_pair,
            crate::commands::get_logs,
            crate::commands::clear_logs,
            crate::commands::get_stats,
            crate::commands::move_download,
            crate::commands::archive_download,
            crate::commands::open_folder,
            crate::commands::open_file
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
