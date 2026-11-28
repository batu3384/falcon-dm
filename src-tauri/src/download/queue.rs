use crate::download::http_client::HttpHeaders;
use crate::download::job::JobContext;
use crate::download::limiter::{CompositeRateLimiter, RateLimiter};
use crate::download::youtube_job::YoutubeJobOptions;
use crate::download::DownloadError;
use crate::settings::{Settings, ALL_SCHEDULE_DAYS};
use crate::storage::{
    models::{Download, DownloadFilter, DownloadStatus},
    Database,
};
use crate::util::{
    is_dash_url, is_googlevideo_url, is_hls_url, is_youtube_watch_url, lock_or_recover,
    split_falcon_format, youtube_page_url_for_download,
};
use chrono::{Datelike, Local, NaiveTime, Weekday};
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

/// True when `now` is outside the allowed download window. Overnight windows
/// belong to the weekday on which they start, so Tuesday 03:00 can be inside
/// Monday 23:00 → 07:00 even when Tuesday itself is disabled.
pub(crate) fn outside_schedule(
    opts: &ScheduleOptions,
    days: &[bool; 7],
    weekday: Weekday,
    now: NaiveTime,
) -> bool {
    if !opts.active {
        return false;
    }
    let parse = |value: &Option<String>| {
        value.as_deref().and_then(|s| NaiveTime::parse_from_str(s, "%H:%M").ok())
    };
    let day_index = weekday.num_days_from_monday() as usize;
    match (parse(&opts.start_time), parse(&opts.stop_time)) {
        (Some(start), Some(stop)) if start <= stop => {
            !days[day_index] || now < start || now >= stop
        }
        // Overnight window (23:00 → 07:00).
        (Some(start), Some(stop)) => {
            if now >= start {
                !days[day_index]
            } else if now < stop {
                !days[(day_index + 6) % 7]
            } else {
                true
            }
        }
        _ => false,
    }
}

#[derive(Debug, PartialEq, Eq)]
pub(crate) enum DownloadRoute {
    Youtube,
    Hls,
    Dash,
    Http,
}

pub(crate) fn route_queued_download(url: &str, referrer: Option<&str>) -> DownloadRoute {
    let (clean, _) = split_falcon_format(url);
    // googlevideo tickets 403 outside the browser — always resolve via watch/referrer.
    if is_googlevideo_url(&clean) {
        return DownloadRoute::Youtube;
    }
    if is_youtube_watch_url(&clean) || youtube_page_url_for_download(url, referrer).is_some() {
        return DownloadRoute::Youtube;
    }
    if is_dash_url(url) {
        return DownloadRoute::Dash;
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
    /// Shared by every connection of this job; changing it throttles live
    /// HTTP/HLS workers without restarting them.
    download_limiter: Arc<RateLimiter>,
}

#[derive(Default)]
struct QueueSlots {
    active: HashMap<i64, ActiveJob>,
    removing: HashMap<i64, usize>,
    shutting_down: bool,
    /// Monotonic worker-generation counter; even a very fast download is observed.
    activity_version: u64,
}

type SharedSlots = Arc<Mutex<QueueSlots>>;

/// Settings the workers read at start.
#[derive(Clone, Default)]
struct QueueOptions {
    proxy: Option<String>,
}

pub struct QueueManager {
    schedule: Mutex<ScheduleOptions>,
    schedule_days: Mutex<[bool; 7]>,
    max_concurrent: AtomicUsize,
    max_connections: AtomicUsize,
    options: Mutex<QueueOptions>,
    limiter: Arc<RateLimiter>,
    slots: SharedSlots,
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
            schedule_days: Mutex::new(ALL_SCHEDULE_DAYS),
            max_concurrent: AtomicUsize::new(3),
            max_connections: AtomicUsize::new(8),
            options: Mutex::new(QueueOptions::default()),
            limiter: Arc::new(RateLimiter::new(0)),
            slots: Arc::new(Mutex::new(QueueSlots::default())),
        }
    }

    pub fn set_schedule(&self, opts: ScheduleOptions) {
        *lock_or_recover(&self.schedule) = opts;
    }

    pub fn get_schedule(&self) -> ScheduleOptions {
        lock_or_recover(&self.schedule).clone()
    }

    pub fn set_schedule_days(&self, days: [bool; 7]) {
        *lock_or_recover(&self.schedule_days) = days;
    }

    pub fn get_schedule_days(&self) -> [bool; 7] {
        *lock_or_recover(&self.schedule_days)
    }

    /// Apply user settings. The speed limit reaches running HTTP/HLS workers at
    /// once; connections and proxy apply from the next start or retry.
    pub fn apply_settings(&self, settings: &Settings) {
        self.max_concurrent
            .store((settings.max_concurrent_downloads as usize).clamp(1, 32), Ordering::SeqCst);
        self.max_connections
            .store((settings.max_connections_per_server as usize).clamp(1, 16), Ordering::SeqCst);
        self.limiter.set_kbps(settings.speed_limit_kbps);
        self.set_schedule_days(settings.schedule_days);
        let trimmed = |value: &str| {
            let value = value.trim();
            (!value.is_empty()).then(|| value.to_string())
        };
        *lock_or_recover(&self.options) =
            QueueOptions { proxy: settings.proxy.as_deref().and_then(trimmed) };
    }

    pub fn max_concurrent(&self) -> usize {
        self.max_concurrent.load(Ordering::SeqCst)
    }

    pub fn speed_limit_kbps(&self) -> u32 {
        self.limiter.kbps()
    }

    pub fn is_active(&self, id: i64) -> bool {
        lock_or_recover(&self.slots).active.contains_key(&id)
    }

    pub fn active_ids(&self) -> Vec<i64> {
        lock_or_recover(&self.slots).active.keys().copied().collect()
    }

    pub(crate) fn activity_version(&self) -> u64 {
        lock_or_recover(&self.slots).activity_version
    }

    fn note_activity(&self) {
        lock_or_recover(&self.slots).activity_version += 1;
    }

    /// Once shutdown begins, no queued row may claim a new worker slot.
    pub(crate) fn begin_shutdown(&self) {
        lock_or_recover(&self.slots).shutting_down = true;
    }

    pub(crate) fn begin_remove(&self, id: i64) {
        *lock_or_recover(&self.slots).removing.entry(id).or_insert(0) += 1;
    }

    pub(crate) fn end_remove(&self, id: i64) {
        let mut slots = lock_or_recover(&self.slots);
        if let Some(count) = slots.removing.get_mut(&id) {
            *count -= 1;
            if *count == 0 {
                slots.removing.remove(&id);
            }
        }
    }

    fn is_shutting_down(&self) -> bool {
        lock_or_recover(&self.slots).shutting_down
    }

    fn claim_active_slot(
        &self,
        id: i64,
        cancel: watch::Sender<bool>,
        download_limiter: Arc<RateLimiter>,
    ) -> bool {
        let mut slots = lock_or_recover(&self.slots);
        if slots.shutting_down || slots.removing.contains_key(&id) || slots.active.contains_key(&id)
        {
            return false;
        }
        slots.active.insert(id, ActiveJob { cancel, stop: None, download_limiter });
        true
    }

    /// Adjust the shared per-download bucket of a running job. False only means
    /// nothing is live right now; the DB write is the durable part for stopped rows.
    pub fn set_download_speed_limit(&self, id: i64, kbps: u32) -> bool {
        let slots = lock_or_recover(&self.slots);
        let Some(job) = slots.active.get(&id) else {
            return false;
        };
        job.download_limiter.set_kbps(kbps);
        true
    }

    /// Ask a running worker to stop. Returns false when nothing is running.
    /// The first reason wins, except that `Remove` overrides everything.
    pub fn request_stop(&self, id: i64, reason: StopReason) -> bool {
        let mut slots = lock_or_recover(&self.slots);
        let Some(job) = slots.active.get_mut(&id) else {
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
        if self.is_shutting_down() {
            return Ok(());
        }
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

        let now = Local::now();
        let schedule_days = self.get_schedule_days();
        if outside_schedule(&schedule, &schedule_days, now.weekday(), now.time()) {
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
        let per_download_limit = row.speed_limit_kbps.unwrap_or(0);
        let download_limiter = Arc::new(RateLimiter::new(per_download_limit));
        let headers = HttpHeaders {
            cookies: row.cookies.clone(),
            referrer: row.referrer.clone(),
            user_agent: row.user_agent.clone(),
            proxy: options.proxy.clone(),
            max_connections,
        };
        let kind = match route_queued_download(&row.url, row.referrer.as_deref()) {
            DownloadRoute::Youtube => {
                let Some(source) = youtube_page_url_for_download(&row.url, row.referrer.as_deref())
                else {
                    // Leaving it Queued would block the head of the queue forever.
                    Self::fail_queued(db, app, id, "not a real media url");
                    return false;
                };
                JobKind::Youtube {
                    url: source,
                    options: YoutubeJobOptions {
                        cookies: row.cookies.clone(),
                        referrer: row.referrer.clone(),
                        proxy: options.proxy.clone(),
                        max_connections,
                        subtitle_tracks: row.subtitle_tracks.clone(),
                    },
                }
            }
            DownloadRoute::Hls => JobKind::Hls { url: row.url.clone(), headers },
            DownloadRoute::Dash => {
                let (url, _) = split_falcon_format(&row.url);
                JobKind::Dash { url, headers }
            }
            DownloadRoute::Http => JobKind::Http { url: row.url.clone(), headers },
        };

        let (cancel, rx) = watch::channel(false);
        // Shutdown, removal, and a still-stopping previous worker all claim through the same lock.
        if !self.claim_active_slot(id, cancel, download_limiter.clone()) {
            return false;
        }
        if !db
            .set_status_if_current(id, &[DownloadStatus::Queued], &DownloadStatus::Downloading)
            .unwrap_or(false)
        {
            lock_or_recover(&self.slots).active.remove(&id);
            return false;
        }
        self.note_activity();
        crate::emit_row(app, db, id);

        let out_path = crate::util::full_file_path(&row.save_path, &row.filename);
        let ctx = JobContext::new(
            id,
            Some(app.clone()),
            Some(db.clone()),
            rx,
            CompositeRateLimiter::new(self.limiter.clone(), download_limiter),
        );
        let guard = JobGuard { id, slots: self.slots.clone(), db: db.clone(), finished: false };
        tokio::spawn(run_job(ctx, kind, out_path, row.attempts, guard));
        true
    }
}

enum JobKind {
    Youtube { url: String, options: YoutubeJobOptions },
    Hls { url: String, headers: HttpHeaders },
    Dash { url: String, headers: HttpHeaders },
    Http { url: String, headers: HttpHeaders },
}

/// Releases the worker slot. If the task unwinds (panic), the row is failed
/// instead of staying `Downloading` with nothing behind it.
struct JobGuard {
    id: i64,
    slots: SharedSlots,
    db: Database,
    finished: bool,
}

impl JobGuard {
    fn stop_reason(&self) -> Option<StopReason> {
        lock_or_recover(&self.slots).active.get(&self.id).and_then(|job| job.stop)
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
        lock_or_recover(&self.slots).active.remove(&self.id);
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
        JobKind::Youtube { url, options } => {
            crate::download::youtube_job::process_youtube(&ctx, &url, &out, options).await
        }
        JobKind::Dash { url, headers } => {
            crate::download::dash::process_dash_stream(&ctx, &url, &out, headers).await
        }
        JobKind::Hls { url, headers } => {
            crate::download::hls::process_hls_stream(&ctx, &url, &out, headers).await
        }
    };

    let Some(db) = ctx.db.as_ref() else {
        guard.finished = true;
        return;
    };
    let app = ctx.app.as_ref();
    let emit_row = |id| {
        if let Some(app) = app {
            crate::emit_row(app, db, id);
        }
    };
    match result {
        Ok(landed) => {
            let size = tokio::fs::metadata(&landed).await.map(|m| m.len()).unwrap_or(0);
            // A row deleted meanwhile keeps its file: never remove what the user may own.
            if db.finish_stream_if_active(id, size).unwrap_or(false) {
                let _ = db.clear_session_cookies(id);
                emit_row(id);
                if let Some(app) = app {
                    crate::notify_download(app, db, id, true);
                }
            }
        }
        Err(error) => {
            let transferred = ctx.progress.transferred();
            let outcome = outcome_for(error, guard.stop_reason(), attempts, transferred);
            // A healthy run proved the link; earlier failures must not consume its next budget.
            if transferred >= HEALTHY_RUN_BYTES
                && matches!(outcome, Outcome::Requeue | Outcome::Leave)
            {
                let _ = db.clear_retry_state(id);
            }
            match outcome {
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
                        emit_row(id);
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
                        emit_row(id);
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
                        emit_row(id);
                        if let Some(app) = app {
                            crate::notify_download(app, db, id, false);
                        }
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
        lock_or_recover(&qm.slots).active.insert(
            id,
            ActiveJob { cancel, stop: None, download_limiter: Arc::new(RateLimiter::new(0)) },
        );
        rx
    }

    fn queued_test_download(status: DownloadStatus) -> Download {
        Download {
            id: None,
            url: "http://127.0.0.1:9/a.bin".into(),
            filename: "a.bin".into(),
            save_path: "/tmp".into(),
            total_size: 0,
            downloaded_size: 0,
            status,
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
            expected_hash: None,
            speed_limit_kbps: None,
            subtitle_tracks: None,
        }
    }

    async fn cancelled_row(transferred: u64, stop: StopReason) -> Download {
        let _loopback = crate::util::allow_loopback_for_tests();
        let db = Database::in_memory().unwrap();
        let id = db.insert_download(&queued_test_download(DownloadStatus::Downloading)).unwrap();
        assert!(db
            .schedule_retry(id, &[DownloadStatus::Downloading], 3, 4_000_000_000, "old failure",)
            .unwrap());
        assert!(db
            .set_status_if_current(id, &[DownloadStatus::Queued], &DownloadStatus::Downloading)
            .unwrap());

        let qm = QueueManager::new();
        let (cancel, rx) = watch::channel(true);
        lock_or_recover(&qm.slots).active.insert(
            id,
            ActiveJob { cancel, stop: Some(stop), download_limiter: Arc::new(RateLimiter::new(0)) },
        );
        let ctx = JobContext::new(
            id,
            None,
            Some(db.clone()),
            rx,
            CompositeRateLimiter::global_only(Arc::new(RateLimiter::new(0))),
        );
        ctx.progress.add_transferred(transferred);
        let guard = JobGuard { id, slots: qm.slots.clone(), db: db.clone(), finished: false };
        let root = std::env::temp_dir().join(format!("falcon-queue-test-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&root).unwrap();
        let out = root.join("a.bin");
        run_job(
            ctx,
            JobKind::Http {
                url: "http://127.0.0.1:9/a.bin".into(),
                headers: HttpHeaders::default(),
            },
            out,
            3,
            guard,
        )
        .await;
        let row = db.get_download(id).unwrap();
        let _ = std::fs::remove_dir_all(root);
        row
    }

    async fn cancelled_requeue_row(transferred: u64) -> Download {
        cancelled_row(transferred, StopReason::Requeue).await
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
            ..Default::default()
        };
        qm.apply_settings(&settings);
        assert_eq!(qm.max_concurrent(), 32);
        assert_eq!(qm.max_connections.load(Ordering::SeqCst), 1);
        assert_eq!(qm.speed_limit_kbps(), 512);
        let options = lock_or_recover(&qm.options).clone();
        assert_eq!(options.proxy, None);
    }

    #[test]
    fn download_speed_limit_update_reaches_the_active_job_limiter() {
        let qm = QueueManager::new();
        let download_limiter = Arc::new(RateLimiter::new(0));
        let (cancel, _rx) = watch::channel(false);
        assert!(qm.claim_active_slot(7, cancel, download_limiter.clone()));

        assert!(qm.set_download_speed_limit(7, 512));
        assert_eq!(download_limiter.kbps(), 512);
        assert!(qm.set_download_speed_limit(7, 0));
        assert_eq!(download_limiter.kbps(), 0, "zero returns the job to the global cap");
        assert!(!qm.set_download_speed_limit(8, 512), "a stopped row has no live limiter");
    }

    #[test]
    fn removal_tombstone_blocks_claiming_until_the_delete_finishes() {
        let qm = QueueManager::new();
        qm.begin_remove(7);
        qm.begin_remove(7);
        let (cancel, _rx) = watch::channel(false);
        assert!(!qm.claim_active_slot(7, cancel, Arc::new(RateLimiter::new(0))));
        assert!(!qm.is_active(7));

        qm.end_remove(7);
        let (cancel, _rx) = watch::channel(false);
        assert!(
            !qm.claim_active_slot(7, cancel, Arc::new(RateLimiter::new(0))),
            "a concurrent remover still owns the tombstone"
        );
        qm.end_remove(7);
        let (cancel, _rx) = watch::channel(false);
        assert!(qm.claim_active_slot(7, cancel, Arc::new(RateLimiter::new(0))));
        assert!(qm.is_active(7));
    }

    #[test]
    fn shutdown_blocks_claiming_new_job_slots() {
        let qm = QueueManager::new();
        qm.begin_shutdown();
        let (cancel, _rx) = watch::channel(false);
        assert!(!qm.claim_active_slot(1, cancel, Arc::new(RateLimiter::new(0))));
        assert!(qm.active_ids().is_empty());
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
        assert_eq!(lock_or_recover(&qm.slots).active[&7].stop, Some(StopReason::Requeue));
        qm.request_stop(7, StopReason::Remove);
        assert_eq!(lock_or_recover(&qm.slots).active[&7].stop, Some(StopReason::Remove));
    }

    #[tokio::test]
    async fn stop_and_wait_returns_after_worker_cleanup() {
        let qm = QueueManager::new();
        let _rx = insert_job(&qm, 7);
        let slots = qm.slots.clone();
        tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(5)).await;
            lock_or_recover(&slots).active.remove(&7);
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
                expected_hash: None,
                speed_limit_kbps: None,
                subtitle_tracks: None,
            })
            .unwrap();
        let qm = QueueManager::new();
        let _rx = insert_job(&qm, id);
        drop(JobGuard { id, slots: qm.slots.clone(), db: db.clone(), finished: false });
        assert!(!qm.is_active(id));
        assert_eq!(db.get_download(id).unwrap().status, DownloadStatus::Failed);
    }

    #[tokio::test]
    async fn healthy_requeued_run_clears_the_retry_budget() {
        let row = cancelled_requeue_row(HEALTHY_RUN_BYTES).await;
        assert_eq!(row.status, DownloadStatus::Queued);
        assert_eq!(row.attempts, 0);
        assert_eq!(row.retry_at, None);
        assert_eq!(row.error_message, None);
    }

    #[tokio::test]
    async fn healthy_shutdown_run_clears_the_retry_budget() {
        let row = cancelled_row(HEALTHY_RUN_BYTES, StopReason::Shutdown).await;
        assert_eq!(row.status, DownloadStatus::Downloading);
        assert_eq!(row.attempts, 0);
        assert_eq!(row.retry_at, None);
        assert_eq!(row.error_message, None);
    }

    #[tokio::test]
    async fn unhealthy_requeued_run_keeps_the_retry_budget() {
        let row = cancelled_requeue_row(0).await;
        assert_eq!(row.status, DownloadStatus::Queued);
        assert_eq!(row.attempts, 3);
        assert_eq!(row.retry_at, Some(4_000_000_000));
        assert_eq!(row.error_message.as_deref(), Some("old failure"));
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
        let all_days = [true; 7];
        let day = ScheduleOptions {
            start_time: Some("09:00".into()),
            stop_time: Some("17:00".into()),
            active: true,
        };
        assert!(outside_schedule(&day, &all_days, chrono::Weekday::Mon, t("08:59")));
        assert!(!outside_schedule(&day, &all_days, chrono::Weekday::Mon, t("09:00")));
        assert!(!outside_schedule(&day, &all_days, chrono::Weekday::Mon, t("16:59")));
        assert!(outside_schedule(&day, &all_days, chrono::Weekday::Mon, t("17:00")));
        let night = ScheduleOptions {
            start_time: Some("23:00".into()),
            stop_time: Some("07:00".into()),
            active: true,
        };
        assert!(!outside_schedule(&night, &all_days, chrono::Weekday::Mon, t("23:30")));
        assert!(!outside_schedule(&night, &all_days, chrono::Weekday::Tue, t("03:00")));
        assert!(outside_schedule(&night, &all_days, chrono::Weekday::Tue, t("07:00")));
        assert!(outside_schedule(&night, &all_days, chrono::Weekday::Tue, t("12:00")));
        let off = ScheduleOptions { active: false, ..night };
        assert!(!outside_schedule(&off, &all_days, chrono::Weekday::Tue, t("12:00")));
    }

    #[test]
    fn daytime_schedule_respects_the_selected_weekday() {
        let t = |s: &str| NaiveTime::parse_from_str(s, "%H:%M").unwrap();
        let monday_only = [true, false, false, false, false, false, false];
        let day = ScheduleOptions {
            start_time: Some("09:00".into()),
            stop_time: Some("17:00".into()),
            active: true,
        };
        assert!(!outside_schedule(&day, &monday_only, chrono::Weekday::Mon, t("10:00")));
        assert!(outside_schedule(&day, &monday_only, chrono::Weekday::Tue, t("10:00")));
        assert!(outside_schedule(&day, &[false; 7], chrono::Weekday::Mon, t("10:00")));
    }

    #[test]
    fn overnight_schedule_belongs_to_the_weekday_it_started_on() {
        let t = |s: &str| NaiveTime::parse_from_str(s, "%H:%M").unwrap();
        let monday_only = [true, false, false, false, false, false, false];
        let night = ScheduleOptions {
            start_time: Some("23:00".into()),
            stop_time: Some("07:00".into()),
            active: true,
        };
        // Monday evening starts the window; Tuesday morning is its tail.
        assert!(!outside_schedule(&night, &monday_only, chrono::Weekday::Mon, t("23:30")));
        assert!(!outside_schedule(&night, &monday_only, chrono::Weekday::Tue, t("03:00")));
        assert!(outside_schedule(&night, &monday_only, chrono::Weekday::Tue, t("07:00")));
        // Tuesday evening and Wednesday morning do not belong to a Monday window.
        assert!(outside_schedule(&night, &monday_only, chrono::Weekday::Tue, t("23:30")));
        assert!(outside_schedule(&night, &monday_only, chrono::Weekday::Wed, t("03:00")));
        // Monday 03:00 is only inside when Sunday is also enabled.
        assert!(outside_schedule(&night, &monday_only, chrono::Weekday::Mon, t("03:00")));
        let sunday_and_monday = [true, false, false, false, false, false, true];
        assert!(!outside_schedule(&night, &sunday_and_monday, chrono::Weekday::Mon, t("03:00")));
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
            DownloadRoute::Dash
        );
        assert_eq!(
            route_queued_download("https://www.youtube.com/watch?v=abc", None),
            DownloadRoute::Youtube
        );
        assert_eq!(
            route_queued_download(
                "https://rr3---sn-abc.googlevideo.com/videoplayback?itag=18&id=xyz",
                Some("https://www.youtube.com/watch?v=abc"),
            ),
            DownloadRoute::Youtube
        );
        let opaque = "https://rr5---sn.googlevideo.com/videoplayback?sabr=1&id=o-APsuIA4T4oPiiO6f7-MNbOU-LO0MDxOui5B37OAnUHZZ";
        assert_eq!(route_queued_download(opaque, None), DownloadRoute::Youtube);
        assert!(
            youtube_page_url_for_download(opaque, None).is_none(),
            "opaque googlevideo without watch page must fail, not stay queued"
        );
    }
}
