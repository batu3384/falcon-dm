use crate::download::http_client::HttpHeaders;
use crate::download::job::JobContext;
use crate::download::limiter::RateLimiter;
use crate::download::ytdlp::YtDlpOptions;
use crate::download::DownloadError;
use crate::settings::Settings;
use crate::storage::{
    models::{Download, DownloadFilter, DownloadStatus},
    Database,
};
use crate::util::{
    is_dash_url, is_googlevideo_url, is_hls_url, is_youtube_watch_url, lock_or_recover,
    split_falcon_format, youtube_page_url_for_download, ytdlp_source_url_for_download,
};
use chrono::{Local, NaiveTime};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::sync::watch;

/// Automatic retries after a transient failure before the row is marked Failed.
pub(crate) const MAX_AUTO_RETRIES: u32 = 5;
/// A run that moved this many bytes proves the link works: the counter restarts.
const HEALTHY_RUN_BYTES: u64 = 4 * 1024 * 1024;

pub(crate) fn retry_delay(attempt: u32) -> Duration {
    Duration::from_secs(match attempt {
        0 | 1 => 3,
        2 => 10,
        3 => 30,
        4 => 60,
        _ => 120,
    })
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ScheduleOptions {
    pub start_time: Option<String>, // "HH:MM"
    pub stop_time: Option<String>,  // "HH:MM"
    pub active: bool,
}

pub fn validate_schedule(opts: &ScheduleOptions) -> Result<(), String> {
    let parse = |value: &str| {
        (value.len() == 5
            && value.as_bytes()[2] == b':'
            && value.as_bytes()[..2].iter().all(u8::is_ascii_digit)
            && value.as_bytes()[3..].iter().all(u8::is_ascii_digit))
        .then(|| NaiveTime::parse_from_str(value, "%H:%M"))
        .transpose()
        .map_err(|_| "Invalid schedule time".to_string())
    };
    let start = opts.start_time.as_deref().map(parse).transpose()?.flatten();
    let stop = opts.stop_time.as_deref().map(parse).transpose()?.flatten();
    if opts.active && (start.is_none() || stop.is_none()) {
        return Err("Active schedule requires start and stop times".into());
    }
    if start.is_some() != stop.is_some() {
        return Err("Schedule requires both start and stop times".into());
    }
    if start == stop && start.is_some() {
        return Err("Schedule start and stop times must differ".into());
    }
    Ok(())
}

/// True when `now` is outside the allowed download window.
pub(crate) fn outside_schedule(opts: &ScheduleOptions, now: NaiveTime) -> bool {
    if !opts.active {
        return false;
    }
    let parse = |value: &Option<String>| {
        value.as_deref().and_then(|s| NaiveTime::parse_from_str(s, "%H:%M").ok())
    };
    match (parse(&opts.start_time), parse(&opts.stop_time)) {
        (Some(start), Some(stop)) if start <= stop => now < start || now >= stop,
        // Overnight window (23:00 → 07:00).
        (Some(start), Some(stop)) => now < start && now >= stop,
        _ => false,
    }
}

#[derive(Debug, PartialEq, Eq)]
pub(crate) enum DownloadRoute {
    YtDlp,
    Hls,
    Http,
}

pub(crate) fn route_queued_download(url: &str, referrer: Option<&str>) -> DownloadRoute {
    let (clean, _) = split_falcon_format(url);
    // googlevideo tickets 403 outside the browser — always yt-dlp via watch/referrer.
    if is_googlevideo_url(&clean) {
        return DownloadRoute::YtDlp;
    }
    if is_youtube_watch_url(&clean) || youtube_page_url_for_download(url, referrer).is_some() {
        return DownloadRoute::YtDlp;
    }
    if is_dash_url(url) {
        return DownloadRoute::YtDlp;
    }
    if is_hls_url(url) {
        DownloadRoute::Hls
    } else {
        DownloadRoute::Http
    }
}

/// Why a running worker was asked to stop. Decides the row's next status.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StopReason {
    /// User pause: the caller already wrote `Paused`.
    Pause,
    /// Schedule window closed or the concurrency limit dropped: back to `Queued`.
    Requeue,
    /// The row is being deleted.
    Remove,
    /// App is quitting: the row stays as is and is requeued on next launch.
    Shutdown,
}

struct ActiveJob {
    cancel: watch::Sender<bool>,
    stop: Option<StopReason>,
}

type ActiveJobs = Arc<Mutex<HashMap<i64, ActiveJob>>>;

/// Settings the workers read at start.
#[derive(Clone, Default)]
struct QueueOptions {
    proxy: Option<String>,
    ytdlp_path: Option<String>,
    ytdlp_use_browser_cookies: bool,
}

pub struct QueueManager {
    schedule: Mutex<ScheduleOptions>,
    max_concurrent: AtomicUsize,
    max_connections: AtomicUsize,
    options: Mutex<QueueOptions>,
    limiter: Arc<RateLimiter>,
    active: ActiveJobs,
}

impl Default for QueueManager {
    fn default() -> Self {
        Self::new()
    }
}

impl QueueManager {
    pub fn new() -> Self {
        Self {
            schedule: Mutex::new(ScheduleOptions {
                start_time: None,
                stop_time: None,
                active: false,
            }),
            max_concurrent: AtomicUsize::new(3),
            max_connections: AtomicUsize::new(8),
            options: Mutex::new(QueueOptions::default()),
            limiter: Arc::new(RateLimiter::new(0)),
            active: Arc::new(Mutex::new(HashMap::new())),
        }
    }

    pub fn set_schedule(&self, opts: ScheduleOptions) {
        *lock_or_recover(&self.schedule) = opts;
    }

    pub fn get_schedule(&self) -> ScheduleOptions {
        lock_or_recover(&self.schedule).clone()
    }

    /// Apply user settings. The speed limit reaches running HTTP/HLS workers at
    /// once; connections and proxy apply from the next start or retry.
    pub fn apply_settings(&self, settings: &Settings) {
        self.max_concurrent
            .store((settings.max_concurrent_downloads as usize).clamp(1, 32), Ordering::SeqCst);
        self.max_connections
            .store((settings.max_connections_per_server as usize).clamp(1, 16), Ordering::SeqCst);
        self.limiter.set_kbps(settings.speed_limit_kbps);
        let trimmed = |value: &str| {
            let value = value.trim();
            (!value.is_empty()).then(|| value.to_string())
        };
        *lock_or_recover(&self.options) = QueueOptions {
            proxy: settings.proxy.as_deref().and_then(trimmed),
            ytdlp_path: trimmed(&settings.ytdlp_path),
            ytdlp_use_browser_cookies: settings.ytdlp_use_browser_cookies,
        };
    }

    pub fn max_concurrent(&self) -> usize {
        self.max_concurrent.load(Ordering::SeqCst)
    }

    pub fn speed_limit_kbps(&self) -> u32 {
        self.limiter.kbps()
    }

    pub fn is_active(&self, id: i64) -> bool {
        lock_or_recover(&self.active).contains_key(&id)
    }

    pub fn active_ids(&self) -> Vec<i64> {
        lock_or_recover(&self.active).keys().copied().collect()
    }

    /// Ask a running worker to stop. Returns false when nothing is running.
    /// The first reason wins, except that `Remove` overrides everything.
    pub fn request_stop(&self, id: i64, reason: StopReason) -> bool {
        let mut active = lock_or_recover(&self.active);
        let Some(job) = active.get_mut(&id) else {
            return false;
        };
        if job.stop.is_none() || reason == StopReason::Remove {
            job.stop = Some(reason);
        }
        job.cancel.send_replace(true);
        true
    }

    fn stop_wait() -> Duration {
        if cfg!(test) {
            Duration::from_millis(120)
        } else {
            Duration::from_secs(15)
        }
    }

    /// Stop a worker and wait until it has released its files.
    pub async fn stop_and_wait(&self, id: i64, reason: StopReason) -> bool {
        if !self.request_stop(id, reason) {
            return true;
        }
        tokio::time::timeout(Self::stop_wait(), async {
            while self.is_active(id) {
                tokio::time::sleep(Duration::from_millis(25)).await;
            }
        })
        .await
        .is_ok()
    }

    /// User pause for one row. Returns false when the row cannot be paused.
    pub async fn pause(&self, db: &Database, app: &tauri::AppHandle, id: i64) -> bool {
        self.request_stop(id, StopReason::Pause);
        let paused = db
            .set_status_error_if_current(
                id,
                &[DownloadStatus::Queued, DownloadStatus::Downloading, DownloadStatus::Merging],
                &DownloadStatus::Paused,
                None,
                Some(0.0),
            )
            .unwrap_or(false);
        if paused {
            crate::emit_row(app, db, id);
        }
        // Let the worker flush its resume state before the caller reports success.
        self.stop_and_wait(id, StopReason::Pause).await;
        paused
    }

    /// Start, stop and requeue workers. Never waits on a worker.
    pub fn tick(&self, db: &Database, app: &tauri::AppHandle) -> Result<(), String> {
        let schedule = self.get_schedule();
        let running = self.active_ids();

        let mut live = db
            .get_downloads(&DownloadFilter {
                statuses: Some(vec![DownloadStatus::Downloading, DownloadStatus::Merging]),
                ..Default::default()
            })
            .map_err(|e| e.to_string())?;
        // A live row without a worker (crash inside a worker) would hang forever.
        for row in &live {
            let Some(id) = row.id else { continue };
            if !running.contains(&id)
                && db
                    .set_status_if_current(
                        id,
                        &[DownloadStatus::Downloading, DownloadStatus::Merging],
                        &DownloadStatus::Queued,
                    )
                    .unwrap_or(false)
            {
                log::warn!("download {id} had no worker; requeued");
                crate::emit_row(app, db, id);
            }
        }
        live.retain(|row| row.id.is_some_and(|id| running.contains(&id)));

        if outside_schedule(&schedule, Local::now().time()) {
            for id in running {
                self.request_stop(id, StopReason::Requeue);
            }
            return Ok(());
        }

        let limit = self.max_concurrent();
        if running.len() > limit {
            // Lowest priority first; among equals the newest row yields.
            live.sort_by_key(|row| (row.priority, std::cmp::Reverse(row.id)));
            for row in live.iter().take(running.len() - limit) {
                if let Some(id) = row.id {
                    self.request_stop(id, StopReason::Requeue);
                }
            }
            return Ok(());
        }

        let free = limit - running.len();
        if free == 0 {
            return Ok(());
        }
        let now_ms = chrono::Utc::now().timestamp_millis();
        // Ask for extra rows: some may still be owned by a stopping worker.
        let candidates =
            db.take_queued((free + running.len()) as i64, now_ms).map_err(|e| e.to_string())?;
        let mut started = 0;
        for row in candidates {
            if started == free {
                break;
            }
            if self.start(db, app, row) {
                started += 1;
            }
        }
        Ok(())
    }

    fn fail_queued(db: &Database, app: &tauri::AppHandle, id: i64, reason: &str) {
        if db
            .set_status_error_if_current(
                id,
                &[DownloadStatus::Queued],
                &DownloadStatus::Failed,
                Some(reason),
                Some(0.0),
            )
            .unwrap_or(false)
        {
            crate::emit_row(app, db, id);
        }
    }

    /// Claim a queued row and spawn its worker. False = not started.
    fn start(&self, db: &Database, app: &tauri::AppHandle, row: Download) -> bool {
        let Some(id) = row.id else {
            return false;
        };
        let options = lock_or_recover(&self.options).clone();
        let max_connections = self.max_connections.load(Ordering::SeqCst);
        let headers = HttpHeaders {
            cookies: row.cookies.clone(),
            referrer: row.referrer.clone(),
            user_agent: row.user_agent.clone(),
            proxy: options.proxy.clone(),
            max_connections,
        };
        let ytdlp = |format: Option<String>| YtDlpOptions {
            cookies: row.cookies.clone(),
            user_agent: row.user_agent.clone(),
            referrer: row.referrer.clone(),
            format,
            proxy: options.proxy.clone(),
            speed_limit_kbps: self.limiter.kbps(),
            ytdlp_path: options.ytdlp_path.clone(),
            use_browser_cookies: options.ytdlp_use_browser_cookies,
            max_connections,
        };
        let kind = match route_queued_download(&row.url, row.referrer.as_deref()) {
            DownloadRoute::YtDlp => {
                let Some(source) = ytdlp_source_url_for_download(&row.url, row.referrer.as_deref())
                else {
                    // Leaving it Queued would block the head of the queue forever.
                    Self::fail_queued(db, app, id, "not a real media url");
                    return false;
                };
                let (url, format) = split_falcon_format(&source);
                let format = format.or_else(|| is_dash_url(&url).then(|| "best".to_string()));
                JobKind::YtDlp { url, options: ytdlp(format) }
            }
            DownloadRoute::Hls => {
                JobKind::Hls { url: row.url.clone(), headers, fallback: ytdlp(None) }
            }
            DownloadRoute::Http => JobKind::Http { url: row.url.clone(), headers },
        };

        let (cancel, rx) = watch::channel(false);
        {
            let mut active = lock_or_recover(&self.active);
            // The previous worker of this row is still shutting down.
            if active.contains_key(&id) {
                return false;
            }
            active.insert(id, ActiveJob { cancel, stop: None });
        }
        if !db
            .set_status_if_current(id, &[DownloadStatus::Queued], &DownloadStatus::Downloading)
            .unwrap_or(false)
        {
            lock_or_recover(&self.active).remove(&id);
            return false;
        }
        crate::emit_row(app, db, id);

        let out_path = crate::util::full_file_path(&row.save_path, &row.filename);
        let ctx =
            JobContext::new(id, Some(app.clone()), Some(db.clone()), rx, self.limiter.clone());
        let guard = JobGuard { id, active: self.active.clone(), db: db.clone(), finished: false };
        tokio::spawn(run_job(ctx, kind, out_path, row.attempts, guard));
        true
    }
}

enum JobKind {
    YtDlp { url: String, options: YtDlpOptions },
    Hls { url: String, headers: HttpHeaders, fallback: YtDlpOptions },
    Http { url: String, headers: HttpHeaders },
}

/// Releases the worker slot. If the task unwinds (panic), the row is failed
/// instead of staying `Downloading` with nothing behind it.
struct JobGuard {
    id: i64,
    active: ActiveJobs,
    db: Database,
    finished: bool,
}

impl JobGuard {
    fn stop_reason(&self) -> Option<StopReason> {
        lock_or_recover(&self.active).get(&self.id).and_then(|job| job.stop)
    }
}

impl Drop for JobGuard {
    fn drop(&mut self) {
        if !self.finished {
            log::error!("download {} worker ended abnormally", self.id);
            let _ = self.db.set_status_error_if_current(
                self.id,
                &[DownloadStatus::Downloading, DownloadStatus::Merging],
                &DownloadStatus::Failed,
                Some("internal error: worker crashed"),
                Some(0.0),
            );
        }
        lock_or_recover(&self.active).remove(&self.id);
    }
}

/// What the row becomes after a worker stopped with an error.
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum Outcome {
    /// Keep whatever status the stopper wrote.
    Leave,
    Requeue,
    Retry {
        attempt: u32,
        delay: Duration,
        message: String,
    },
    Fail(String),
}

pub(crate) fn outcome_for(
    error: DownloadError,
    stop: Option<StopReason>,
    attempts: u32,
    transferred: u64,
) -> Outcome {
    match error {
        DownloadError::Cancelled => match stop {
            Some(StopReason::Requeue) => Outcome::Requeue,
            _ => Outcome::Leave,
        },
        DownloadError::Transient(message) => {
            let attempt = if transferred >= HEALTHY_RUN_BYTES { 1 } else { attempts + 1 };
            if attempt > MAX_AUTO_RETRIES {
                Outcome::Fail(message)
            } else {
                Outcome::Retry { attempt, delay: retry_delay(attempt), message }
            }
        }
        DownloadError::Fatal(message) => Outcome::Fail(message),
        DownloadError::NeedsYtDlp => Outcome::Fail("ERR_YTDLP_MISSING".into()),
    }
}

async fn run_job(
    ctx: JobContext,
    kind: JobKind,
    out_path: PathBuf,
    attempts: u32,
    mut guard: JobGuard,
) {
    let id = ctx.id;
    let out = out_path.to_string_lossy().to_string();
    let result = match kind {
        JobKind::Http { url, headers } => {
            crate::download::http::process_http(&ctx, &url, &out, headers).await
        }
        JobKind::YtDlp { url, options } => {
            crate::download::ytdlp::process_ytdlp(&ctx, &url, &out, options).await
        }
        JobKind::Hls { url, headers, fallback } => {
            match crate::download::hls::process_hls_stream(&ctx, &url, &out, headers).await {
                Err(DownloadError::NeedsYtDlp) => {
                    crate::download::ytdlp::process_ytdlp(&ctx, &url, &out, fallback).await
                }
                other => other,
            }
        }
    };

    let (Some(app), Some(db)) = (ctx.app.as_ref(), ctx.db.as_ref()) else {
        guard.finished = true;
        return;
    };
    match result {
        Ok(landed) => {
            let size = tokio::fs::metadata(&landed).await.map(|m| m.len()).unwrap_or(0);
            // A row deleted meanwhile keeps its file: never remove what the user may own.
            if db.finish_stream_if_active(id, size).unwrap_or(false) {
                let _ = db.clear_session_cookies(id);
                crate::emit_row(app, db, id);
                crate::notify_download(app, db, id, true);
            }
        }
        Err(error) => {
            let transferred = ctx.progress.transferred();
            match outcome_for(error, guard.stop_reason(), attempts, transferred) {
                Outcome::Leave => {}
                Outcome::Requeue => {
                    if db
                        .set_status_if_current(
                            id,
                            &[DownloadStatus::Downloading, DownloadStatus::Merging],
                            &DownloadStatus::Queued,
                        )
                        .unwrap_or(false)
                    {
                        crate::emit_row(app, db, id);
                    }
                }
                Outcome::Retry { attempt, delay, message } => {
                    log::warn!(
                        "download {id}: {message} — retry {attempt}/{MAX_AUTO_RETRIES} in {}s",
                        delay.as_secs()
                    );
                    let retry_at = chrono::Utc::now().timestamp_millis() + delay.as_millis() as i64;
                    if db
                        .schedule_retry(
                            id,
                            &[DownloadStatus::Downloading, DownloadStatus::Merging],
                            attempt,
                            retry_at,
                            &message,
                        )
                        .unwrap_or(false)
                    {
                        crate::emit_row(app, db, id);
                    }
                }
                Outcome::Fail(message) => {
                    log::error!("download {id} failed: {message}");
                    if db
                        .set_status_error_if_current(
                            id,
                            &[DownloadStatus::Downloading, DownloadStatus::Merging],
                            &DownloadStatus::Failed,
                            Some(&message),
                            Some(0.0),
                        )
                        .unwrap_or(false)
                    {
                        let _ = db.clear_session_cookies(id);
                        crate::emit_row(app, db, id);
                        crate::notify_download(app, db, id, false);
                    }
                }
            }
        }
    }
    guard.finished = true;
}

#[cfg(test)]
mod tests {
    use super::*;

    fn insert_job(qm: &QueueManager, id: i64) -> watch::Receiver<bool> {
        let (cancel, rx) = watch::channel(false);
        lock_or_recover(&qm.active).insert(id, ActiveJob { cancel, stop: None });
        rx
    }

    #[test]
    fn defaults_and_settings_apply() {
        let qm = QueueManager::new();
        assert_eq!(qm.max_concurrent(), 3);
        assert!(!qm.get_schedule().active);
        let settings = Settings {
            max_concurrent_downloads: 99,
            max_connections_per_server: 0,
            speed_limit_kbps: 512,
            proxy: Some("  ".into()),
            ytdlp_path: " /opt/homebrew/bin/yt-dlp ".into(),
            ..Default::default()
        };
        qm.apply_settings(&settings);
        assert_eq!(qm.max_concurrent(), 32);
        assert_eq!(qm.max_connections.load(Ordering::SeqCst), 1);
        assert_eq!(qm.speed_limit_kbps(), 512);
        let options = lock_or_recover(&qm.options).clone();
        assert_eq!(options.proxy, None);
        assert_eq!(options.ytdlp_path.as_deref(), Some("/opt/homebrew/bin/yt-dlp"));
    }

    #[test]
    fn request_stop_signals_and_keeps_first_reason() {
        let qm = QueueManager::new();
        assert!(!qm.request_stop(999, StopReason::Pause));
        let rx = insert_job(&qm, 7);
        assert!(qm.request_stop(7, StopReason::Requeue));
        assert!(*rx.borrow());
        assert!(qm.is_active(7), "the slot is released by the worker, not by the stopper");
        qm.request_stop(7, StopReason::Pause);
        assert_eq!(lock_or_recover(&qm.active)[&7].stop, Some(StopReason::Requeue));
        qm.request_stop(7, StopReason::Remove);
        assert_eq!(lock_or_recover(&qm.active)[&7].stop, Some(StopReason::Remove));
    }

    #[tokio::test]
    async fn stop_and_wait_returns_after_worker_cleanup() {
        let qm = QueueManager::new();
        let _rx = insert_job(&qm, 7);
        let active = qm.active.clone();
        tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(5)).await;
            lock_or_recover(&active).remove(&7);
        });
        assert!(qm.stop_and_wait(7, StopReason::Remove).await);
        assert!(!qm.is_active(7));
        assert!(qm.stop_and_wait(7, StopReason::Remove).await, "idle row is already stopped");
    }

    #[tokio::test]
    async fn stop_and_wait_times_out_on_a_stuck_worker() {
        let qm = QueueManager::new();
        let _rx = insert_job(&qm, 7);
        assert!(!qm.stop_and_wait(7, StopReason::Remove).await);
        assert!(qm.is_active(7));
    }

    #[test]
    fn job_guard_fails_the_row_when_the_worker_dies() {
        let db = Database::in_memory().unwrap();
        let id = db
            .insert_download(&Download {
                id: None,
                url: "https://example.com/a.bin".into(),
                filename: "a.bin".into(),
                save_path: "/tmp".into(),
                total_size: 0,
                downloaded_size: 0,
                status: DownloadStatus::Downloading,
                category: crate::storage::models::DownloadCategory::Other,
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
            })
            .unwrap();
        let qm = QueueManager::new();
        let _rx = insert_job(&qm, id);
        drop(JobGuard { id, active: qm.active.clone(), db: db.clone(), finished: false });
        assert!(!qm.is_active(id));
        assert_eq!(db.get_download(id).unwrap().status, DownloadStatus::Failed);
    }

    #[test]
    fn outcomes_follow_stop_reason_and_retry_budget() {
        use DownloadError::*;
        assert_eq!(outcome_for(Cancelled, Some(StopReason::Pause), 0, 0), Outcome::Leave);
        assert_eq!(outcome_for(Cancelled, Some(StopReason::Requeue), 0, 0), Outcome::Requeue);
        assert_eq!(outcome_for(Cancelled, Some(StopReason::Shutdown), 0, 0), Outcome::Leave);
        assert_eq!(outcome_for(Cancelled, None, 0, 0), Outcome::Leave);
        assert_eq!(
            outcome_for(Transient("HTTP 503".into()), None, 0, 0),
            Outcome::Retry { attempt: 1, delay: retry_delay(1), message: "HTTP 503".into() }
        );
        assert_eq!(
            outcome_for(Transient("x".into()), None, MAX_AUTO_RETRIES, 0),
            Outcome::Fail("x".into()),
            "the retry budget must end"
        );
        // A run that moved real data starts the budget again.
        assert_eq!(
            outcome_for(Transient("x".into()), None, MAX_AUTO_RETRIES, HEALTHY_RUN_BYTES),
            Outcome::Retry { attempt: 1, delay: retry_delay(1), message: "x".into() }
        );
        assert_eq!(
            outcome_for(Fatal("HTTP 404".into()), None, 0, 0),
            Outcome::Fail("HTTP 404".into())
        );
        assert!(retry_delay(1) < retry_delay(3) && retry_delay(3) < retry_delay(9));
    }

    #[test]
    fn schedule_window_handles_daytime_and_overnight() {
        let t = |s: &str| NaiveTime::parse_from_str(s, "%H:%M").unwrap();
        let day = ScheduleOptions {
            start_time: Some("09:00".into()),
            stop_time: Some("17:00".into()),
            active: true,
        };
        assert!(outside_schedule(&day, t("08:59")));
        assert!(!outside_schedule(&day, t("09:00")));
        assert!(!outside_schedule(&day, t("16:59")));
        assert!(outside_schedule(&day, t("17:00")));
        let night = ScheduleOptions {
            start_time: Some("23:00".into()),
            stop_time: Some("07:00".into()),
            active: true,
        };
        assert!(!outside_schedule(&night, t("23:30")));
        assert!(!outside_schedule(&night, t("03:00")));
        assert!(outside_schedule(&night, t("07:00")));
        assert!(outside_schedule(&night, t("12:00")));
        let off = ScheduleOptions { active: false, ..night };
        assert!(!outside_schedule(&off, t("12:00")));
    }

    #[test]
    fn overnight_schedule_is_valid_but_equal_or_malformed_is_not() {
        let opts = |start: &str, stop: &str| ScheduleOptions {
            start_time: Some(start.into()),
            stop_time: Some(stop.into()),
            active: true,
        };
        assert!(validate_schedule(&opts("23:00", "06:00")).is_ok());
        assert!(validate_schedule(&opts("06:00", "06:00")).is_err());
        assert!(validate_schedule(&opts("6:00", "07:00")).is_err());
        assert!(validate_schedule(&opts("24:00", "07:00")).is_err());
    }

    #[test]
    fn route_uses_path_not_query_for_hls() {
        assert_eq!(
            route_queued_download("https://cdn.example.com/video.mp4?x=.m3u8", None),
            DownloadRoute::Http
        );
        assert_eq!(
            route_queued_download("https://cdn.example.com/live.m3u8", None),
            DownloadRoute::Hls
        );
        assert_eq!(
            route_queued_download("https://cdn.example.com/stream.mpd", None),
            DownloadRoute::YtDlp
        );
        assert_eq!(
            route_queued_download("https://www.youtube.com/watch?v=abc", None),
            DownloadRoute::YtDlp
        );
        assert_eq!(
            route_queued_download(
                "https://rr3---sn-abc.googlevideo.com/videoplayback?itag=18&id=xyz",
                Some("https://www.youtube.com/watch?v=abc"),
            ),
            DownloadRoute::YtDlp
        );
        let opaque = "https://rr5---sn.googlevideo.com/videoplayback?sabr=1&id=o-APsuIA4T4oPiiO6f7-MNbOU-LO0MDxOui5B37OAnUHZZ";
        assert_eq!(route_queued_download(opaque, None), DownloadRoute::YtDlp);
        assert!(
            ytdlp_source_url_for_download(opaque, None).is_none(),
            "opaque googlevideo without watch page must fail, not stay queued"
        );
    }
}
