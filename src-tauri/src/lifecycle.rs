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
    menu::{CheckMenuItem, Menu, MenuItem},
    tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent},
    AppHandle, Emitter, Manager,
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

/// Delete orphaned partial files and stream work dirs under the download
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
            if let Some(id) = crate::download::youtube_job::parse_run_dir_name(&name) {
                // Legacy yt-dlp builds used `-<uuid>` suffixed dirs nothing can resume.
                let current = name == format!(".falcon-dm-yt-{id}");
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

fn is_extension_cors_origin(origin: &str) -> bool {
    crate::extension_id_from_origin(origin).is_some_and(crate::is_valid_extension_id)
}

fn spawn_local_api(app_handle: tauri::AppHandle) {
    tauri::async_runtime::spawn(async move {
        use tauri::http::HeaderValue;
        use tower_http::cors::{AllowOrigin, CorsLayer};

        let cors = CorsLayer::new()
            .allow_origin(AllowOrigin::predicate(|origin: &HeaderValue, _| {
                origin.to_str().is_ok_and(is_extension_cors_origin)
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
            .route("/api/downloads/all", get(crate::local_api::handle_downloads_all))
            .route("/api/stats", get(crate::local_api::handle_stats))
            .route("/api/pause", post(crate::local_api::handle_pause))
            .route("/api/resume", post(crate::local_api::handle_resume))
            .route("/api/remove", post(crate::local_api::handle_remove))
            .route("/api/pause_all", post(crate::local_api::handle_pause_all))
            .route("/api/resume_all", post(crate::local_api::handle_resume_all))
            .route("/api/speed_limit", post(crate::local_api::handle_speed_limit))
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

/// Idle-sleep prevention while bytes are moving. `caffeinate -w <own pid>`
/// ties the assertion to this process, so even a crash cannot leave the Mac
/// awake forever; a clean finish is handled by killing the child.
#[cfg(target_os = "macos")]
#[derive(Default)]
struct SleepAssertion {
    child: Option<std::process::Child>,
    // Spawn failures are not retried until the queue goes fully idle once:
    // a missing caffeinate must not spin a failing spawn every tick.
    spawn_failed: bool,
}

#[cfg(target_os = "macos")]
impl SleepAssertion {
    fn update(&mut self, downloading: bool) {
        if !downloading {
            self.spawn_failed = false;
            if let Some(mut child) = self.child.take() {
                let _ = child.kill();
                let _ = child.wait();
            }
            return;
        }
        // Reap an unexpected exit so the assertion can be raised again.
        if let Some(child) = self.child.as_mut() {
            if matches!(child.try_wait(), Ok(Some(_))) {
                self.child = None;
            }
        }
        if self.child.is_none() && !self.spawn_failed {
            let spawn = std::process::Command::new("/usr/bin/caffeinate")
                .arg("-i")
                .arg("-w")
                .arg(std::process::id().to_string())
                .stdin(std::process::Stdio::null())
                .stdout(std::process::Stdio::null())
                .stderr(std::process::Stdio::null())
                .spawn();
            match spawn {
                Ok(child) => {
                    log::info!("idle sleep prevented while downloads are active");
                    self.child = Some(child);
                }
                Err(e) => {
                    self.spawn_failed = true;
                    log::warn!("could not spawn caffeinate; sleep is not prevented: {e}");
                }
            }
        }
    }
}

/// Fires once when observed worker activity reaches a fully empty queue.
#[derive(Default)]
struct CompletionDrainDetector {
    activity_version: u64,
    armed: bool,
}

impl CompletionDrainDetector {
    fn observe(
        &mut self,
        activity_version: u64,
        active_count: u64,
        queued_count: u64,
        paused_count: u64,
    ) -> bool {
        if activity_version != self.activity_version {
            self.activity_version = activity_version;
            self.armed = true;
        }
        if active_count > 0 || !self.armed {
            return false;
        }
        self.armed = false;
        queued_count == 0 && paused_count == 0
    }
}

fn validate_completion_script(
    raw_path: &str,
    allowed_roots: &[std::path::PathBuf],
) -> Result<std::path::PathBuf, String> {
    let raw_path = raw_path.trim();
    if raw_path.is_empty() {
        return Err("Completion script path is empty".into());
    }
    let expanded = util::expand_tilde(raw_path);
    if !expanded.is_absolute() {
        return Err("Completion script path must be absolute".into());
    }
    let path =
        expanded.canonicalize().map_err(|e| format!("Completion script does not exist: {e}"))?;
    let metadata = std::fs::metadata(&path).map_err(|e| e.to_string())?;
    if !metadata.is_file() {
        return Err("Completion script must be a file".into());
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        if metadata.permissions().mode() & 0o111 == 0 {
            return Err("Completion script is not executable".into());
        }
    }
    let allowed = allowed_roots
        .iter()
        .any(|root| root.canonicalize().ok().is_some_and(|canonical| path.starts_with(canonical)));
    if !allowed {
        return Err("Completion script is outside the allowed locations".into());
    }
    Ok(path)
}

fn completion_script_roots() -> Vec<std::path::PathBuf> {
    let mut roots = Vec::new();
    if let Some(home) = dirs::home_dir() {
        roots.push(home);
    }
    roots.push(std::path::PathBuf::from("/opt/homebrew/bin"));
    roots.push(std::path::PathBuf::from("/usr/local/bin"));
    roots
}

/// Reap the child without blocking the queue loop; nothing is routed through a shell.
fn spawn_detached(label: &str, command: &mut std::process::Command) -> Result<(), String> {
    let mut child = command
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .map_err(|e| format!("Could not start {label}: {e}"))?;
    let label = label.to_string();
    std::thread::spawn(move || match child.wait() {
        Ok(status) if !status.success() => log::warn!("{label} exited with {status}"),
        Err(e) => log::warn!("{label} could not be reaped: {e}"),
        _ => {}
    });
    Ok(())
}

fn execute_completion_action(settings: &Settings) -> Result<(), String> {
    match settings.completion_action.as_str() {
        "none" => Ok(()),
        "sleep" => {
            #[cfg(target_os = "macos")]
            {
                let mut command = std::process::Command::new("/usr/bin/osascript");
                command.arg("-e").arg("tell application \"System Events\" to sleep");
                spawn_detached("system sleep", &mut command)
            }
            #[cfg(not(target_os = "macos"))]
            {
                Err("Sleep completion action is only supported on macOS".into())
            }
        }
        "script" => {
            let script = validate_completion_script(
                &settings.completion_script,
                &completion_script_roots(),
            )?;
            let mut command = std::process::Command::new(script);
            spawn_detached("completion script", &mut command)
        }
        other => Err(format!("Unknown completion action: {other}")),
    }
}

fn spawn_queue_loop(app_handle: tauri::AppHandle) {
    tauri::async_runtime::spawn(async move {
        let mut interval = tokio::time::interval(std::time::Duration::from_millis(500));
        interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        #[cfg(target_os = "macos")]
        let mut sleep_assertion = SleepAssertion::default();
        let mut completion_drain = CompletionDrainDetector::default();
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

            let state = app_handle.state::<AppState>();
            let active_count = state.queue.active_ids().len() as u64;
            #[cfg(target_os = "macos")]
            sleep_assertion.update(active_count > 0);
            let Ok(stats) = state.db.download_stats() else {
                log::warn!("completion action check could not read download stats");
                continue;
            };
            // Drain semantics: Completed and Failed both count as done; Queued or
            // Paused rows mean work remains, consume that drain, and only new worker
            // activity can arm the one-shot action again.
            if completion_drain.observe(
                state.queue.activity_version(),
                active_count,
                stats.queued,
                stats.paused,
            ) {
                let settings = util::lock_or_recover(&state.settings).clone();
                if settings.completion_action != "none" {
                    log::info!("queue drained; running completion action");
                    if let Err(e) = execute_completion_action(&settings) {
                        log::warn!("completion action did not start: {e}");
                    }
                }
            }
        }
    });
}

const TRAY_ID: &str = "main";

/// Decimal like `DEFAULT_LIMIT_KBPS` in src/lib/speedLimit.ts: exactly 1 MB/s.
const DEFAULT_SPEED_LIMIT_KBPS: u32 = 1000;

/// `(new limit, new memory)` for one speed-limit toggle. Turning off captures
/// the live value; turning on restores the memory or the 1 MB/s default.
fn speed_limit_toggle_next(current: u32, last: u32) -> (u32, u32) {
    if current > 0 {
        (0, current)
    } else {
        let restore = if last > 0 { last } else { DEFAULT_SPEED_LIMIT_KBPS };
        (restore, restore)
    }
}

/// Tray mirror of the toolbar snail toggle: same settings field, same live
/// application through `QueueManager::apply_settings`. The frontend picks the
/// change up on its stats poll.
fn apply_speed_limit_toggle(app: &AppHandle) {
    let state = app.state::<AppState>();
    let mut settings = util::lock_or_recover(&state.settings).clone();
    let (next, last) =
        speed_limit_toggle_next(settings.speed_limit_kbps, settings.speed_limit_last_kbps);
    settings.speed_limit_kbps = next;
    settings.speed_limit_last_kbps = last;
    if let Err(e) = settings.save(&app_data_dir()) {
        log::error!("tray speed limit toggle could not save settings: {e}");
        return;
    }
    state.queue.apply_settings(&settings);
    *util::lock_or_recover(&state.settings) = settings;
}

fn rebuild_tray_menu(app: &AppHandle) -> Result<(), String> {
    let lang = util::lock_or_recover(&app.state::<AppState>().ui_lang).clone();
    let menu = tray_menu(app, &lang).map_err(|e| e.to_string())?;
    app.tray_by_id(TRAY_ID)
        .ok_or_else(|| "tray icon missing".to_string())?
        .set_menu(Some(menu))
        .map_err(|e| e.to_string())
}

/// The checkmark must follow limit changes from anywhere (Settings pane,
/// toolbar toggle, tray). There is no settings-changed hook, so a cheap poll
/// rebuilds the menu only when the state actually flips.
fn spawn_tray_speed_limit_sync(app_handle: tauri::AppHandle) {
    tauri::async_runtime::spawn(async move {
        let mut interval = tokio::time::interval(std::time::Duration::from_secs(1));
        interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        let mut last: Option<bool> = None;
        loop {
            interval.tick().await;
            let state = app_handle.state::<AppState>();
            let on = util::lock_or_recover(&state.settings).speed_limit_kbps > 0;
            if last == Some(on) {
                continue;
            }
            last = Some(on);
            if let Err(e) = rebuild_tray_menu(&app_handle) {
                log::warn!("tray speed limit sync could not rebuild the menu: {e}");
            }
        }
    });
}

fn locale_is_turkish(lang: &str) -> bool {
    lang.trim().split(['-', '_', '.']).next().is_some_and(|tag| tag.eq_ignore_ascii_case("tr"))
}

fn tray_menu(app: &AppHandle, lang: &str) -> tauri::Result<Menu<tauri::Wry>> {
    let tr = locale_is_turkish(lang);
    let item = |id: &str, en: &str, turkish: &str| {
        MenuItem::with_id(app, id, if tr { turkish } else { en }, true, None::<&str>)
    };
    let limit_on = util::lock_or_recover(&app.state::<AppState>().settings).speed_limit_kbps > 0;
    let show_i = item("show", "Show Falcon DM", "Falcon DM'i Göster")?;
    let new_i = item("new", "New Download", "Yeni İndirme")?;
    let pause_i = item("pause", "Pause All", "Tümünü Duraklat")?;
    let resume_i = item("resume", "Resume All", "Tümünü Sürdür")?;
    let speed_i = CheckMenuItem::with_id(
        app,
        "speedlimit",
        if tr { "Hız Sınırı" } else { "Speed Limit" },
        true,
        limit_on,
        None::<&str>,
    )?;
    let quit_i = item("quit", "Quit", "Çıkış")?;
    Menu::with_items(app, &[&show_i, &new_i, &pause_i, &resume_i, &speed_i, &quit_i])
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
            "new" => {
                crate::show_main_window(app);
                let _ = app.emit("open-new-download", ());
            }
            "pause" => {
                let app = app.clone();
                tauri::async_runtime::spawn(async move {
                    crate::pause_all(&app).await;
                });
            }
            "resume" => {
                crate::resume_all(app);
            }
            "speedlimit" => {
                let app = app.clone();
                tauri::async_runtime::spawn(async move {
                    apply_speed_limit_toggle(&app);
                    if let Err(e) = rebuild_tray_menu(&app) {
                        log::warn!("tray speed limit toggle could not rebuild the menu: {e}");
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
    // Manual theme choice reaches the NSWindow before first paint — the
    // frontend re-applies it too, but only after loading settings over IPC.
    let startup_theme = settings.theme.clone();
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
                let dark = match startup_theme.as_str() {
                    "dark" => Some(true),
                    "light" => Some(false),
                    _ => None,
                };
                if dark.is_some() {
                    let _ = apply_ns_window_appearance(&window, dark);
                }
            }

            setup_tray(app, &system_lang)?;
            spawn_tray_speed_limit_sync(app_handle.clone());
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
            crate::commands::set_download_speed_limit,
            crate::commands::get_settings,
            crate::commands::save_settings,
            crate::commands::get_extension_status,
            crate::commands::approve_extension_pair,
            crate::commands::reject_extension_pair,
            crate::commands::revoke_extension,
            crate::commands::get_logs,
            crate::commands::clear_logs,
            crate::commands::get_stats,
            crate::commands::move_download,
            crate::commands::archive_download,
            crate::commands::refresh_download_url,
            crate::commands::open_folder,
            crate::commands::open_file,
            crate::commands::check_for_updates,
            crate::commands::read_clipboard,
            set_window_appearance,
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
    app_handle.state::<AppState>().queue.begin_shutdown();
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

/// Force the window's NSAppearance to the in-app theme override.
///
/// The sidebar vibrancy material resolves against the window appearance, not
/// the webview's CSS, so a manual dark/light choice must reach AppKit — with
/// `None` (system) the appearance is cleared and follows the OS again.
/// Sync command: Tauri runs those on the main thread, which AppKit requires.
#[cfg(target_os = "macos")]
fn apply_ns_window_appearance(
    window: &tauri::WebviewWindow,
    dark: Option<bool>,
) -> Result<(), String> {
    use objc2_app_kit::{
        NSAppearance, NSAppearanceCustomization, NSAppearanceNameAqua, NSAppearanceNameDarkAqua,
        NSWindow,
    };
    let ptr = window.ns_window().map_err(|e| e.to_string())?;
    let Some(ptr) = std::ptr::NonNull::new(ptr) else {
        return Err("main window has no native NSWindow".into());
    };
    // SAFETY: `ns_window` returns the live NSWindow of this webview window, and
    // callers run on the main thread (app setup, or a sync command — Tauri
    // executes those on the main thread), as AppKit mutation requires.
    let ns_window: &NSWindow = unsafe { ptr.cast().as_ref() };
    let appearance = match dark {
        // SAFETY: the extern statics are immutable, process-wide constants.
        Some(true) => NSAppearance::appearanceNamed(unsafe { NSAppearanceNameDarkAqua }),
        Some(false) => NSAppearance::appearanceNamed(unsafe { NSAppearanceNameAqua }),
        None => None,
    };
    ns_window.setAppearance(appearance.as_deref());
    Ok(())
}

#[tauri::command]
pub fn set_window_appearance(app: AppHandle, theme: Option<String>) -> Result<(), String> {
    let dark = match theme.as_deref() {
        Some("dark") => Some(true),
        Some("light") => Some(false),
        _ => None,
    };
    #[cfg(target_os = "macos")]
    {
        let window = app.get_webview_window("main").ok_or("main window missing")?;
        apply_ns_window_appearance(&window, dark)
    }
    #[cfg(not(target_os = "macos"))]
    {
        let _ = (app, dark);
        Ok(())
    }
}

#[tauri::command]
pub fn set_ui_language(app: AppHandle, lang: String) -> Result<(), String> {
    *util::lock_or_recover(&app.state::<AppState>().ui_lang) = lang;
    rebuild_tray_menu(&app)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extension_cors_origin_requires_scheme_and_valid_id() {
        assert!(is_extension_cors_origin("chrome-extension://aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"));
        assert!(is_extension_cors_origin("moz-extension://e3225586-81a0-47c3-8612-d95fb0c2a609"));
        assert!(!is_extension_cors_origin("moz-extension://not-a-valid-id"));
        assert!(!is_extension_cors_origin("https://example.com"));
        assert!(!is_extension_cors_origin(
            "moz-extension://e3225586-81a0-47c3-8612-d95fb0c2a609/path"
        ));
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn ns_appearance_names_resolve() {
        use objc2_app_kit::{NSAppearance, NSAppearanceNameAqua, NSAppearanceNameDarkAqua};
        // SAFETY: the extern statics are immutable, process-wide constants.
        assert!(NSAppearance::appearanceNamed(unsafe { NSAppearanceNameDarkAqua }).is_some());
        assert!(NSAppearance::appearanceNamed(unsafe { NSAppearanceNameAqua }).is_some());
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn sleep_assertion_spawns_once_and_releases() {
        let mut assertion = SleepAssertion::default();
        assertion.update(false);
        assert!(assertion.child.is_none());
        assertion.update(true);
        let pid = assertion.child.as_ref().map(std::process::Child::id);
        assert!(pid.is_some(), "caffeinate should run while downloads are active");
        assertion.update(true);
        assert_eq!(
            assertion.child.as_ref().map(std::process::Child::id),
            pid,
            "a steady state must not respawn caffeinate every tick"
        );
        assertion.update(false);
        assert!(assertion.child.is_none(), "idle queue releases the sleep assertion");
    }

    #[test]
    fn locale_is_turkish_matches_tr_tags() {
        assert!(locale_is_turkish("tr"));
        assert!(locale_is_turkish("tr-TR"));
        assert!(locale_is_turkish("tr_TR.UTF-8"));
        assert!(!locale_is_turkish("en"));
        assert!(!locale_is_turkish("en_US"));
    }

    #[test]
    fn speed_limit_toggle_captures_and_restores_memory() {
        // Off with no memory: the 1 MB/s default, which becomes the memory.
        assert_eq!(speed_limit_toggle_next(0, 0), (1000, 1000));
        // Off with a memory: restore exactly that value.
        assert_eq!(speed_limit_toggle_next(0, 500), (500, 500));
        // On: zero the limit but capture the live value for the next restore.
        assert_eq!(speed_limit_toggle_next(500, 0), (0, 500));
        assert_eq!(speed_limit_toggle_next(250, 500), (0, 250));
        // A full round trip returns to the remembered value.
        let (off, memory) = speed_limit_toggle_next(500, 0);
        assert_eq!(speed_limit_toggle_next(off, memory), (500, 500));
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
        let kept_run = root.join(".falcon-dm-yt-7");
        let stale_run = root.join(".falcon-dm-yt-3");
        // Legacy yt-dlp generations are never resumable by the native engine —
        // their child layout differs — so cleanup removes them even for live ids.
        let legacy_run = root.join(".falcon-dm-ytdlp-7");
        let legacy_uuid_run = root.join(".falcon-dm-ytdlp-7-123e4567-e89b-12d3-a456-426614174000");
        for dir in [&kept_run, &stale_run, &legacy_run, &legacy_uuid_run] {
            std::fs::create_dir_all(dir).unwrap();
        }

        cleanup_stale_download_temps(std::slice::from_ref(&root), &keep);
        assert!(kept_part.exists() && kept_state.exists() && kept_run.exists());
        assert!(!stale_part.exists() && !stale_run.exists() && !legacy_run.exists());
        assert!(!legacy_uuid_run.exists(), "legacy run dirs are always stale");
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

    #[test]
    fn completion_drain_fires_once_per_active_generation() {
        let mut detector = CompletionDrainDetector::default();
        assert!(!detector.observe(0, 0, 0, 0), "a queue that was never active must not fire");
        assert!(!detector.observe(1, 1, 0, 0));
        assert!(detector.observe(1, 0, 0, 0), "the first empty drain fires");
        assert!(!detector.observe(1, 0, 0, 0), "the same drain must not fire twice");
        assert!(!detector.observe(2, 1, 0, 0));
        assert!(detector.observe(2, 0, 0, 0), "new activity re-arms the detector");
    }

    #[test]
    fn queued_or_paused_work_consumes_a_drain_without_firing() {
        let mut queued = CompletionDrainDetector::default();
        assert!(!queued.observe(1, 1, 0, 0));
        assert!(!queued.observe(1, 0, 1, 0), "queued work means the queue has not drained");
        assert!(
            !queued.observe(1, 0, 0, 0),
            "removing that queued row later must not retroactively fire"
        );

        let mut paused = CompletionDrainDetector::default();
        assert!(!paused.observe(1, 1, 0, 0));
        assert!(!paused.observe(1, 0, 0, 1), "manually paused work suppresses the action");
        assert!(!paused.observe(1, 0, 0, 0));
    }

    #[cfg(unix)]
    fn executable_script(root: &std::path::Path, name: &str) -> std::path::PathBuf {
        use std::os::unix::fs::PermissionsExt;
        let script = root.join(name);
        std::fs::write(&script, "#!/bin/sh\nexit 0\n").unwrap();
        std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755)).unwrap();
        script
    }

    #[cfg(unix)]
    #[test]
    fn completion_script_validation_accepts_an_executable_inside_an_allowed_root() {
        let root = std::env::temp_dir().join(format!("falcon-script-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&root).unwrap();
        let script = executable_script(&root, "done.sh");
        let validated =
            validate_completion_script(script.to_str().unwrap(), std::slice::from_ref(&root))
                .unwrap();
        assert_eq!(validated, script.canonicalize().unwrap());
        std::fs::remove_dir_all(root).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn completion_script_validation_rejects_missing_nonexecutable_and_outside_paths() {
        use std::os::unix::fs::PermissionsExt;
        let root = std::env::temp_dir().join(format!("falcon-script-{}", uuid::Uuid::new_v4()));
        let outside = std::env::temp_dir().join(format!("falcon-script-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&root).unwrap();
        std::fs::create_dir_all(&outside).unwrap();

        assert!(validate_completion_script("", std::slice::from_ref(&root)).is_err());
        assert!(validate_completion_script(
            root.join("missing.sh").to_str().unwrap(),
            std::slice::from_ref(&root),
        )
        .is_err());

        let not_executable = root.join("plain.sh");
        std::fs::write(&not_executable, "#!/bin/sh\n").unwrap();
        std::fs::set_permissions(&not_executable, std::fs::Permissions::from_mode(0o644)).unwrap();
        assert!(validate_completion_script(
            not_executable.to_str().unwrap(),
            std::slice::from_ref(&root)
        )
        .is_err());

        let outside_script = executable_script(&outside, "outside.sh");
        assert!(validate_completion_script(
            outside_script.to_str().unwrap(),
            std::slice::from_ref(&root)
        )
        .is_err());
        assert!(validate_completion_script(root.to_str().unwrap(), std::slice::from_ref(&root))
            .is_err());

        std::fs::remove_dir_all(root).unwrap();
        std::fs::remove_dir_all(outside).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn completion_script_validation_follows_symlinks_before_the_root_check() {
        use std::os::unix::fs::symlink;
        let root = std::env::temp_dir().join(format!("falcon-script-{}", uuid::Uuid::new_v4()));
        let outside = std::env::temp_dir().join(format!("falcon-script-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&root).unwrap();
        std::fs::create_dir_all(&outside).unwrap();
        let outside_script = executable_script(&outside, "outside.sh");
        let link = root.join("linked.sh");
        symlink(&outside_script, &link).unwrap();
        assert!(validate_completion_script(link.to_str().unwrap(), std::slice::from_ref(&root))
            .is_err());
        std::fs::remove_dir_all(root).unwrap();
        std::fs::remove_dir_all(outside).unwrap();
    }
}
