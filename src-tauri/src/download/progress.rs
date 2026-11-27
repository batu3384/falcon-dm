//! Throttled progress reporting shared by every worker.
//!
//! Workers only bump atomics on the hot path. A ticker turns them into at most
//! two UI events per second and one DB write every two seconds, with a smoothed
//! speed that reflects the last few seconds instead of the lifetime average.

use crate::storage::{models::DownloadStatus, Database};
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tauri::{AppHandle, Emitter};
// tokio's clock, so paused-time tests drive the ticker deterministically.
use tokio::time::Instant;

const TICK: Duration = Duration::from_millis(500);
const DB_EVERY_TICKS: u32 = 4;
/// Time constant of the speed average.
const SPEED_TAU_SECS: f64 = 2.0;

struct Inner {
    id: i64,
    app: Option<AppHandle>,
    db: Option<Database>,
    downloaded: AtomicU64,
    /// No byte count is authoritative until a worker restores or receives bytes.
    /// Before that, publishing zero would erase whatever the row already had.
    counts_known: AtomicBool,
    /// Bytes received by this run only. Speed comes from here, so restoring
    /// `downloaded` on resume does not look like a burst of traffic.
    transferred: AtomicU64,
    total: AtomicU64,
    connections: AtomicU32,
    merging: AtomicBool,
    speed: Mutex<f64>,
}

#[derive(Clone)]
pub struct Progress {
    inner: Arc<Inner>,
}

/// Next smoothed speed from the bytes seen in the last `dt` seconds.
pub(crate) fn smooth_speed(previous: f64, delta_bytes: u64, dt: f64) -> f64 {
    if dt <= 0.0 {
        return previous;
    }
    let instant = delta_bytes as f64 / dt;
    if previous <= 0.0 {
        return instant;
    }
    let alpha = 1.0 - (-dt / SPEED_TAU_SECS).exp();
    previous + alpha * (instant - previous)
}

impl Progress {
    pub fn new(app: Option<&AppHandle>, db: Option<Database>, id: i64) -> Self {
        Self {
            inner: Arc::new(Inner {
                id,
                app: app.cloned(),
                db,
                downloaded: AtomicU64::new(0),
                counts_known: AtomicBool::new(false),
                transferred: AtomicU64::new(0),
                total: AtomicU64::new(0),
                connections: AtomicU32::new(1),
                merging: AtomicBool::new(false),
                speed: Mutex::new(0.0),
            }),
        }
    }

    pub fn id(&self) -> i64 {
        self.inner.id
    }

    pub fn add(&self, bytes: u64) {
        self.inner.downloaded.fetch_add(bytes, Ordering::Relaxed);
        self.inner.transferred.fetch_add(bytes, Ordering::Relaxed);
        if bytes > 0 {
            self.inner.counts_known.store(true, Ordering::Release);
        }
    }

    /// Live traffic that does not map 1:1 to file bytes (yt-dlp reports totals).
    pub fn add_transferred(&self, bytes: u64) {
        self.inner.transferred.fetch_add(bytes, Ordering::Relaxed);
    }

    pub fn set_downloaded(&self, bytes: u64) {
        self.inner.downloaded.store(bytes, Ordering::Relaxed);
        self.inner.counts_known.store(true, Ordering::Release);
    }

    pub fn downloaded(&self) -> u64 {
        self.inner.downloaded.load(Ordering::Relaxed)
    }

    /// Bytes this run received over the network.
    pub fn transferred(&self) -> u64 {
        self.inner.transferred.load(Ordering::Relaxed)
    }

    /// 0 = unknown size.
    pub fn set_total(&self, bytes: u64) {
        self.inner.total.store(bytes, Ordering::Relaxed);
    }

    pub fn total(&self) -> u64 {
        self.inner.total.load(Ordering::Relaxed)
    }

    pub fn set_connections(&self, connections: u32) {
        self.inner.connections.store(connections, Ordering::Relaxed);
    }

    /// Bytes are complete; the worker is muxing or moving the file.
    pub fn set_merging(&self) {
        self.inner.merging.store(true, Ordering::Relaxed);
        *crate::util::lock_or_recover(&self.inner.speed) = 0.0;
        self.publish(true);
    }

    fn status(&self) -> DownloadStatus {
        if self.inner.merging.load(Ordering::Relaxed) {
            DownloadStatus::Merging
        } else {
            DownloadStatus::Downloading
        }
    }

    fn publish(&self, write_db: bool) {
        if !self.inner.counts_known.load(Ordering::Acquire) {
            return;
        }
        let downloaded = self.downloaded();
        let total = self.total();
        let speed = *crate::util::lock_or_recover(&self.inner.speed);
        let status = self.status();
        if write_db {
            if let Some(db) = &self.inner.db {
                let _ = db.update_download_progress(
                    self.inner.id,
                    downloaded,
                    (total > 0).then_some(total.max(downloaded)),
                    speed,
                    &status,
                );
            }
        }
        if let Some(app) = &self.inner.app {
            let _ = app.emit(
                "download-progress",
                serde_json::json!({
                    "id": self.inner.id,
                    "downloaded_size": downloaded,
                    // 0 keeps the UI in "size unknown" mode instead of showing 100%.
                    "total_size": if total > 0 { total.max(downloaded) } else { 0 },
                    "speed": speed,
                    "status": status.as_str(),
                    "connections": self.inner.connections.load(Ordering::Relaxed),
                }),
            );
        }
    }

    /// Start periodic reporting. Dropping the guard stops it and persists the
    /// final byte counts when this run produced authoritative counts.
    pub fn start(&self) -> ProgressTicker {
        let progress = self.clone();
        let handle = tokio::spawn(async move {
            let mut last_bytes = progress.inner.transferred.load(Ordering::Relaxed);
            let mut last_at = Instant::now();
            let mut tick = 0u32;
            let mut interval = tokio::time::interval(TICK);
            interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
            interval.tick().await;
            loop {
                interval.tick().await;
                let now = Instant::now();
                let bytes = progress.inner.transferred.load(Ordering::Relaxed);
                let dt = now.duration_since(last_at).as_secs_f64();
                if !progress.inner.merging.load(Ordering::Relaxed) {
                    let mut speed = crate::util::lock_or_recover(&progress.inner.speed);
                    *speed = smooth_speed(*speed, bytes.saturating_sub(last_bytes), dt);
                    // Below 1 B/s is noise from the decay tail.
                    if *speed < 1.0 {
                        *speed = 0.0;
                    }
                }
                last_bytes = bytes;
                last_at = now;
                tick = tick.wrapping_add(1);
                progress.publish(tick.is_multiple_of(DB_EVERY_TICKS));
            }
        });
        ProgressTicker { progress: self.clone(), handle }
    }
}

pub struct ProgressTicker {
    progress: Progress,
    handle: tokio::task::JoinHandle<()>,
}

impl Drop for ProgressTicker {
    fn drop(&mut self) {
        self.handle.abort();
        *crate::util::lock_or_recover(&self.progress.inner.speed) = 0.0;
        if self.progress.inner.counts_known.load(Ordering::Acquire) {
            if let Some(db) = &self.progress.inner.db {
                let total = self.progress.total();
                let downloaded = self.progress.downloaded();
                let _ = db.update_download_progress(
                    self.progress.inner.id,
                    downloaded,
                    (total > 0).then_some(total.max(downloaded)),
                    0.0,
                    &self.progress.status(),
                );
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::storage::models::{Download, DownloadCategory};

    #[test]
    fn speed_follows_recent_throughput_not_lifetime_average() {
        // 10 s at 1 MB/s, then the link stalls.
        let mut speed = 0.0;
        for _ in 0..20 {
            speed = smooth_speed(speed, 500_000, 0.5);
        }
        assert!((speed - 1_000_000.0).abs() < 20_000.0, "{speed}");
        for _ in 0..12 {
            speed = smooth_speed(speed, 0, 0.5);
        }
        assert!(speed < 60_000.0, "stall must show within a few seconds, got {speed}");
    }

    #[test]
    fn first_sample_is_used_as_is() {
        assert_eq!(smooth_speed(0.0, 1000, 0.5), 2000.0);
        assert_eq!(smooth_speed(123.0, 1000, 0.0), 123.0);
    }

    #[tokio::test(start_paused = true)]
    async fn restored_bytes_do_not_count_as_speed() {
        let progress = Progress::new(None, None, 1);
        let _ticker = progress.start();
        progress.set_downloaded(500_000_000);
        tokio::time::sleep(Duration::from_millis(1600)).await;
        assert_eq!(*crate::util::lock_or_recover(&progress.inner.speed), 0.0);
        progress.add(1_000_000);
        tokio::time::sleep(Duration::from_millis(600)).await;
        let speed = *crate::util::lock_or_recover(&progress.inner.speed);
        assert!(speed > 100_000.0 && speed < 5_000_000.0, "{speed}");
        assert_eq!(progress.downloaded(), 501_000_000);
    }

    #[tokio::test]
    async fn dropping_the_ticker_persists_bytes_for_resume() {
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
            })
            .unwrap();
        let progress = Progress::new(None, Some(db.clone()), id);
        progress.set_total(1000);
        let ticker = progress.start();
        progress.add(400);
        drop(ticker);
        let row = db.get_download(id).unwrap();
        assert_eq!(row.downloaded_size, 400);
        assert_eq!(row.total_size, 1000);
        assert_eq!(row.speed, 0.0);
    }
}
