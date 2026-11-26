//! One token bucket shared by every HTTP and HLS worker, so the configured
//! speed limit is a global cap and a Settings change applies to running jobs.

use crate::download::DownloadError;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Mutex;
use std::time::{Duration, Instant};
use tokio::sync::watch;

/// Resolves when the watch flips to `true`. Never resolves if the sender is
/// dropped without cancelling, so it is safe as a `select!` branch.
pub async fn cancelled(rx: &mut watch::Receiver<bool>) {
    loop {
        if *rx.borrow() {
            return;
        }
        if rx.changed().await.is_err() {
            std::future::pending::<()>().await;
        }
    }
}

/// Sleep that ends early with `Cancelled`.
pub async fn sleep_or_cancel(
    wait: Duration,
    cancel: &watch::Receiver<bool>,
) -> Result<(), DownloadError> {
    let mut rx = cancel.clone();
    tokio::select! {
        _ = tokio::time::sleep(wait) => Ok(()),
        _ = cancelled(&mut rx) => Err(DownloadError::Cancelled),
    }
}

struct Bucket {
    tokens: f64,
    refilled: Instant,
}

pub struct RateLimiter {
    bytes_per_sec: AtomicU64,
    bucket: Mutex<Bucket>,
}

impl RateLimiter {
    pub fn new(kbps: u32) -> Self {
        Self {
            bytes_per_sec: AtomicU64::new(u64::from(kbps) * 1024),
            bucket: Mutex::new(Bucket { tokens: 0.0, refilled: Instant::now() }),
        }
    }

    /// 0 = unlimited. Clears accumulated debt so the new limit applies at once.
    pub fn set_kbps(&self, kbps: u32) {
        self.bytes_per_sec.store(u64::from(kbps) * 1024, Ordering::Relaxed);
        let mut bucket = crate::util::lock_or_recover(&self.bucket);
        bucket.tokens = 0.0;
        bucket.refilled = Instant::now();
    }

    pub fn kbps(&self) -> u32 {
        (self.bytes_per_sec.load(Ordering::Relaxed) / 1024) as u32
    }

    /// Debt model: the bytes are already received, so charge them and report
    /// how long the caller must wait before reading more.
    fn charge(&self, bytes: u64) -> Option<Duration> {
        let rate = self.bytes_per_sec.load(Ordering::Relaxed) as f64;
        if rate <= 0.0 {
            return None;
        }
        let mut bucket = crate::util::lock_or_recover(&self.bucket);
        let now = Instant::now();
        let elapsed = now.duration_since(bucket.refilled).as_secs_f64();
        bucket.refilled = now;
        // Burst is capped at one second of traffic.
        bucket.tokens = (bucket.tokens + elapsed * rate).min(rate) - bytes as f64;
        (bucket.tokens < 0.0).then(|| Duration::from_secs_f64(-bucket.tokens / rate))
    }

    pub async fn acquire(
        &self,
        bytes: u64,
        cancel: &watch::Receiver<bool>,
    ) -> Result<(), DownloadError> {
        let Some(wait) = self.charge(bytes) else {
            return Ok(());
        };
        // Cap one sleep so "unlimited" or a higher limit is picked up quickly.
        sleep_or_cancel(wait.min(Duration::from_secs(2)), cancel).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn unlimited_never_waits() {
        let limiter = RateLimiter::new(0);
        let (_tx, rx) = watch::channel(false);
        let started = Instant::now();
        limiter.acquire(50 * 1024 * 1024, &rx).await.unwrap();
        assert!(started.elapsed() < Duration::from_millis(50));
    }

    #[test]
    fn charge_reports_debt_shared_across_callers() {
        let limiter = RateLimiter::new(100); // 102_400 B/s
                                             // Two "downloads" each push 102_400 bytes: together they owe ~2 s.
        let first = limiter.charge(102_400).expect("first caller waits");
        let second = limiter.charge(102_400).expect("second caller waits longer");
        assert!(first.as_secs_f64() > 0.9 && first.as_secs_f64() < 1.1, "{first:?}");
        assert!(second.as_secs_f64() > 1.9 && second.as_secs_f64() < 2.1, "{second:?}");
    }

    #[test]
    fn raising_the_limit_clears_debt() {
        let limiter = RateLimiter::new(1);
        assert!(limiter.charge(10 * 1024 * 1024).is_some());
        limiter.set_kbps(0);
        assert!(limiter.charge(10 * 1024 * 1024).is_none());
        assert_eq!(limiter.kbps(), 0);
    }

    #[tokio::test]
    async fn acquire_stops_on_cancel() {
        let limiter = RateLimiter::new(1);
        let (tx, rx) = watch::channel(false);
        tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(20)).await;
            let _ = tx.send(true);
        });
        let started = Instant::now();
        let result = limiter.acquire(10 * 1024 * 1024, &rx).await;
        assert_eq!(result, Err(DownloadError::Cancelled));
        assert!(started.elapsed() < Duration::from_secs(1));
    }

    #[tokio::test]
    async fn cancelled_ignores_a_dropped_sender() {
        let (tx, mut rx) = watch::channel(false);
        drop(tx);
        let outcome = tokio::time::timeout(Duration::from_millis(30), cancelled(&mut rx)).await;
        assert!(outcome.is_err(), "dropped sender must not look like a cancel");
    }
}
