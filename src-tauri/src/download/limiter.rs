//! One token bucket shared by every HTTP and HLS worker, so the configured
//! speed limit is a global cap and a Settings change applies to running jobs.

use crate::download::DownloadError;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::sync::watch;
use tokio::time::Instant;

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
    /// `kbps` is decimal: the UI shows 1000-based units everywhere, so the
    /// "KB/s" the user types means 1000 bytes per second.
    pub fn new(kbps: u32) -> Self {
        Self {
            bytes_per_sec: AtomicU64::new(u64::from(kbps) * 1000),
            bucket: Mutex::new(Bucket { tokens: 0.0, refilled: Instant::now() }),
        }
    }

    /// 0 = unlimited. Clears accumulated debt so the new limit applies at once.
    pub fn set_kbps(&self, kbps: u32) {
        self.bytes_per_sec.store(u64::from(kbps) * 1000, Ordering::Relaxed);
        let mut bucket = crate::util::lock_or_recover(&self.bucket);
        bucket.tokens = 0.0;
        bucket.refilled = Instant::now();
    }

    pub fn kbps(&self) -> u32 {
        (self.bytes_per_sec.load(Ordering::Relaxed) / 1000) as u32
    }

    fn refill_bucket(bucket: &mut Bucket, rate: f64) {
        let now = Instant::now();
        let elapsed = now.duration_since(bucket.refilled).as_secs_f64();
        bucket.refilled = now;
        // Burst is capped at one second of traffic.
        bucket.tokens = (bucket.tokens + elapsed * rate).min(rate);
    }

    /// Debt model: the bytes are already received, so charge them and report
    /// how long the caller must wait before reading more.
    fn charge(&self, bytes: u64) -> Option<Duration> {
        let rate = self.bytes_per_sec.load(Ordering::Relaxed) as f64;
        if rate <= 0.0 {
            return None;
        }
        let mut bucket = crate::util::lock_or_recover(&self.bucket);
        Self::refill_bucket(&mut bucket, rate);
        bucket.tokens -= bytes as f64;
        (bucket.tokens < 0.0).then(|| Duration::from_secs_f64(-bucket.tokens / rate))
    }

    fn debt_wait(&self) -> Option<Duration> {
        let rate = self.bytes_per_sec.load(Ordering::Relaxed) as f64;
        if rate <= 0.0 {
            return None;
        }
        let mut bucket = crate::util::lock_or_recover(&self.bucket);
        Self::refill_bucket(&mut bucket, rate);
        (bucket.tokens < 0.0).then(|| Duration::from_secs_f64(-bucket.tokens / rate))
    }

    pub async fn acquire(
        &self,
        bytes: u64,
        cancel: &watch::Receiver<bool>,
    ) -> Result<(), DownloadError> {
        let Some(mut wait) = self.charge(bytes) else {
            return Ok(());
        };
        loop {
            // Re-check long debts so "unlimited" or a higher limit is picked up quickly.
            sleep_or_cancel(wait.min(Duration::from_secs(2)), cancel).await?;
            let Some(remaining) = self.debt_wait() else {
                return Ok(());
            };
            wait = remaining;
        }
    }
}

fn longest_wait(first: Option<Duration>, second: Option<Duration>) -> Option<Duration> {
    match (first, second) {
        (Some(first), Some(second)) => Some(first.max(second)),
        (Some(wait), None) | (None, Some(wait)) => Some(wait),
        (None, None) => None,
    }
}

/// Per-download and global token buckets around one job. Both are charged for
/// the same already-received bytes before the worker sleeps; the debts run
/// concurrently, so the wait is their maximum rather than their sum.
#[derive(Clone)]
pub struct CompositeRateLimiter {
    global: Arc<RateLimiter>,
    per_download: Arc<RateLimiter>,
}

impl CompositeRateLimiter {
    pub fn new(global: Arc<RateLimiter>, per_download: Arc<RateLimiter>) -> Self {
        Self { global, per_download }
    }

    /// Global-only composition for tests and legacy call sites: a zeroed
    /// per-download bucket means "unlimited here, still follow the global cap".
    pub fn global_only(global: Arc<RateLimiter>) -> Self {
        Self::new(global, Arc::new(RateLimiter::new(0)))
    }

    /// Charge both buckets synchronously. Splitting this into two awaited
    /// `acquire` calls would serialize the sleeps and under-deliver bandwidth.
    fn charge(&self, bytes: u64) -> Option<Duration> {
        let download_wait = self.per_download.charge(bytes);
        let global_wait = self.global.charge(bytes);
        longest_wait(download_wait, global_wait)
    }

    fn debt_wait(&self) -> Option<Duration> {
        longest_wait(self.per_download.debt_wait(), self.global.debt_wait())
    }

    pub async fn acquire(
        &self,
        bytes: u64,
        cancel: &watch::Receiver<bool>,
    ) -> Result<(), DownloadError> {
        let Some(mut wait) = self.charge(bytes) else {
            return Ok(());
        };
        loop {
            // Re-check long debts so "unlimited" or a higher limit is picked up quickly.
            sleep_or_cancel(wait.min(Duration::from_secs(2)), cancel).await?;
            let Some(remaining) = self.debt_wait() else {
                return Ok(());
            };
            wait = remaining;
        }
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
        let limiter = RateLimiter::new(100); // 100_000 B/s
                                             // Two "downloads" each push 100_000 bytes: together they owe ~2 s.
        let first = limiter.charge(100_000).expect("first caller waits");
        let second = limiter.charge(100_000).expect("second caller waits longer");
        assert!(first.as_secs_f64() > 0.9 && first.as_secs_f64() < 1.1, "{first:?}");
        assert!(second.as_secs_f64() > 1.9 && second.as_secs_f64() < 2.1, "{second:?}");
    }

    #[test]
    fn composite_charge_waits_for_the_stricter_bucket_not_the_sum() {
        let global = std::sync::Arc::new(RateLimiter::new(2)); // 2000 B/s
        let download = std::sync::Arc::new(RateLimiter::new(1)); // 1000 B/s
        let composite = CompositeRateLimiter::new(global.clone(), download.clone());

        let wait = composite.charge(1000).expect("both buckets are limited");
        assert!(wait.as_secs_f64() > 0.9 && wait.as_secs_f64() < 1.1, "{wait:?}");
        assert!(
            wait.as_secs_f64() < 1.4,
            "charging the buckets one after another would sum their debts ({wait:?})"
        );
        assert!(download.debt_wait().is_some(), "the per-download bucket must be charged");
        assert!(global.debt_wait().is_some(), "the global bucket must also be charged");
    }

    #[test]
    fn composite_charge_lets_a_stricter_global_cap_dominate() {
        let global = std::sync::Arc::new(RateLimiter::new(1)); // 1000 B/s
        let download = std::sync::Arc::new(RateLimiter::new(2)); // 2000 B/s
        let composite = CompositeRateLimiter::new(global, download);

        let wait = composite.charge(1000).expect("both buckets are limited");
        assert!(wait.as_secs_f64() > 0.9 && wait.as_secs_f64() < 1.1, "{wait:?}");
        assert!(wait.as_secs_f64() < 1.4, "the debts must overlap, not add");
    }

    #[test]
    fn composite_charge_treats_zero_per_download_limit_as_follow_global() {
        let global = std::sync::Arc::new(RateLimiter::new(1));
        let download = std::sync::Arc::new(RateLimiter::new(0));
        let composite = CompositeRateLimiter::new(global, download);

        let wait = composite.charge(1000).expect("the global cap still applies");
        assert!(wait.as_secs_f64() > 0.9 && wait.as_secs_f64() < 1.1, "{wait:?}");
    }

    #[test]
    fn kbps_is_decimal_end_to_end() {
        let limiter = RateLimiter::new(1);
        assert_eq!(limiter.bytes_per_sec.load(Ordering::Relaxed), 1000);
        assert_eq!(limiter.kbps(), 1);
        limiter.set_kbps(512);
        assert_eq!(limiter.bytes_per_sec.load(Ordering::Relaxed), 512_000);
        assert_eq!(limiter.kbps(), 512);
    }

    #[test]
    fn raising_the_limit_clears_debt() {
        let limiter = RateLimiter::new(1);
        assert!(limiter.charge(10 * 1024 * 1024).is_some());
        limiter.set_kbps(0);
        assert!(limiter.charge(10 * 1024 * 1024).is_none());
        assert_eq!(limiter.kbps(), 0);
    }

    #[tokio::test(start_paused = true)]
    async fn sustained_acquire_throughput_converges_to_the_configured_rate() {
        let limiter = std::sync::Arc::new(RateLimiter::new(1)); // 1000 B/s
        let (_tx, rx) = watch::channel(false);
        let workers: Vec<_> = (0..2)
            .map(|_| {
                let worker_limiter = limiter.clone();
                let rx = rx.clone();
                tokio::spawn(async move {
                    for _ in 0..2 {
                        worker_limiter.acquire(4000, &rx).await.unwrap();
                    }
                })
            })
            .collect();
        tokio::task::yield_now().await;

        for _ in 0..90 {
            tokio::time::advance(Duration::from_millis(100)).await;
            tokio::task::yield_now().await;
        }
        assert!(
            workers.iter().all(|worker| !worker.is_finished()),
            "16000 bytes at 1000 B/s need about 16 seconds"
        );

        for _ in 0..75 {
            tokio::time::advance(Duration::from_millis(100)).await;
            tokio::task::yield_now().await;
        }
        assert!(
            workers.iter().all(|worker| worker.is_finished()),
            "the debt should be paid shortly after 16 seconds"
        );
        for worker in workers {
            worker.await.unwrap();
        }
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
