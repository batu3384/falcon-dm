use crate::download::engine::{Aria2Engine, Aria2Options};
use crate::download::hls::HlsHeaders;
use crate::storage::{
    models::{DownloadFilter, DownloadStatus},
    Database,
};
use crate::util::{
    is_hls_url, lock_or_recover, sanitize_header_value, split_falcon_format,
    youtube_page_url_for_download,
};
use chrono::{Local, NaiveTime};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use tauri::Emitter;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ScheduleOptions {
    pub start_time: Option<String>, // "HH:MM"
    pub stop_time: Option<String>,  // "HH:MM"
    pub active: bool,
}

async fn determine_connection_limits(
    client: &reqwest::Client,
    url: &str,
    max_cap: u32,
    cookies: Option<&str>,
    referrer: Option<&str>,
    user_agent: Option<&str>,
) -> (u32, u32) {
    if url.starts_with("magnet:") || url.ends_with(".torrent") {
        return (max_cap.min(16), max_cap.min(16));
    }

    let mut req = client.head(url);
    if let Some(c) = cookies {
        if !c.is_empty() {
            req = req.header("Cookie", sanitize_header_value(c));
        }
    }
    if let Some(r) = referrer {
        if !r.is_empty() {
            req = req.header("Referer", sanitize_header_value(r));
        }
    }
    if let Some(ua) = user_agent {
        if !ua.is_empty() {
            req = req.header("User-Agent", sanitize_header_value(ua));
        }
    }

    if let Ok(res) = req.send().await {
        if let Some(len) = res.content_length() {
            if len < 10 * 1024 * 1024 {
                return (2.min(max_cap), 2.min(max_cap));
            }
            if len < 100 * 1024 * 1024 {
                return (8.min(max_cap), 8.min(max_cap));
            }
            return (max_cap.min(16), max_cap.min(16));
        }
    }
    (4.min(max_cap), 4.min(max_cap))
}

pub struct QueueManager {
    pub schedule: Arc<Mutex<ScheduleOptions>>,
    pub max_concurrent: Arc<AtomicUsize>,
    pub max_connections: Arc<AtomicUsize>,
    pub active_stream_tasks: Arc<Mutex<HashMap<i64, tokio::sync::watch::Sender<bool>>>>,
    /// HEAD probe cache: url -> (connection count, probed_at). TTL 5 min.
    pub head_probe_cache: Arc<Mutex<HashMap<String, (u32, Instant)>>>,
    /// Shared reqwest client for HEAD probes (connection-pooled). Previously each
    /// probe built a fresh Client → a new TLS handshake every time.
    probe_client: reqwest::Client,
}

impl Default for QueueManager {
    fn default() -> Self {
        Self::new()
    }
}

impl QueueManager {
    pub fn new() -> Self {
        Self {
            schedule: Arc::new(Mutex::new(ScheduleOptions {
                start_time: None,
                stop_time: None,
                active: false,
            })),
            max_concurrent: Arc::new(AtomicUsize::new(3)),
            max_connections: Arc::new(AtomicUsize::new(16)),
            active_stream_tasks: Arc::new(Mutex::new(HashMap::new())),
            head_probe_cache: Arc::new(Mutex::new(HashMap::new())),
            probe_client: reqwest::Client::builder()
                .timeout(Duration::from_secs(5))
                .build()
                .unwrap_or_else(|_| reqwest::Client::new()),
        }
    }

    pub fn set_schedule(&self, opts: ScheduleOptions) {
        let mut lock = lock_or_recover(&self.schedule);
        *lock = opts;
    }

    pub fn get_schedule(&self) -> ScheduleOptions {
        lock_or_recover(&self.schedule).clone()
    }

    pub fn set_concurrent_downloads(&self, limit: usize) {
        self.max_concurrent.store(limit.max(1), Ordering::SeqCst);
    }

    pub fn set_max_connections(&self, limit: usize) {
        self.max_connections.store(limit.clamp(1, 16), Ordering::SeqCst);
    }

    pub fn cancel_stream(&self, id: i64) -> bool {
        if let Some(tx) = lock_or_recover(&self.active_stream_tasks).remove(&id) {
            let _ = tx.send(true);
            true
        } else {
            false
        }
    }

    /// Maximum entries kept in the HEAD probe cache. Beyond this the oldest
    /// entries are evicted to bound memory (previously the HashMap grew forever
    /// — a slow memory leak in long-running sessions).
    const HEAD_CACHE_CAP: usize = 256;

    /// Cached connection count for `url` if fresh (5 min TTL), else None.
    fn cached_conn_limit(&self, url: &str) -> Option<u32> {
        let cache = lock_or_recover(&self.head_probe_cache);
        let (val, ts) = cache.get(url)?;
        (ts.elapsed() < Duration::from_secs(300)).then_some(*val)
    }

    /// Sweep expired + excess entries so the cache stays bounded.
    fn evict_stale_probe_entries(cache: &Arc<Mutex<HashMap<String, (u32, Instant)>>>) {
        let mut cache = lock_or_recover(cache);
        let ttl = Duration::from_secs(300);
        cache.retain(|_, (_, ts)| ts.elapsed() < ttl);
        if cache.len() > Self::HEAD_CACHE_CAP {
            // Drop the oldest entries by probe time.
            let mut by_age: Vec<_> = cache.iter().map(|(k, (_, ts))| (k.clone(), *ts)).collect();
            by_age.sort_by_key(|(_, ts)| *ts);
            let excess = cache.len() - Self::HEAD_CACHE_CAP;
            for (k, _) in by_age.into_iter().take(excess) {
                cache.remove(&k);
            }
        }
    }

    /// Decide (split, max_connections) for `url` without blocking the tick.
    /// Cache hit → cached value; miss → default (max_cap) now + best-effort background HEAD probe.
    // ponytail: first tick after a miss uses the default; the probe fills the cache for subsequent ticks.
    fn conn_limits(
        &self,
        url: &str,
        max_cap: u32,
        cookies: Option<&str>,
        referrer: Option<&str>,
        user_agent: Option<&str>,
    ) -> (u32, u32) {
        if let Some(c) = self.cached_conn_limit(url) {
            return (c, c);
        }
        let cache = self.head_probe_cache.clone();
        let probe_client = self.probe_client.clone();
        let url_owned = url.to_string();
        let c = cookies.map(str::to_string);
        let r = referrer.map(str::to_string);
        let u = user_agent.map(str::to_string);
        tokio::spawn(async move {
            let (split, _) = determine_connection_limits(
                &probe_client,
                &url_owned,
                max_cap,
                c.as_deref(),
                r.as_deref(),
                u.as_deref(),
            )
            .await;
            {
                let mut m = lock_or_recover(&cache);
                m.insert(url_owned, (split, Instant::now()));
            }
            // Bound the cache after each insert (evict expired + overflow).
            QueueManager::evict_stale_probe_entries(&cache);
        });
        (max_cap, max_cap)
    }

    pub async fn tick(
        &self,
        db: &Database,
        engine: &Aria2Engine,
        app_handle: tauri::AppHandle,
    ) -> Result<(), String> {
        let opts = self.get_schedule();
        let mut is_scheduled_paused = false;

        if opts.active {
            let now = Local::now().time();
            let start =
                opts.start_time.as_ref().and_then(|s| NaiveTime::parse_from_str(s, "%H:%M").ok());
            let stop =
                opts.stop_time.as_ref().and_then(|s| NaiveTime::parse_from_str(s, "%H:%M").ok());

            if let (Some(start), Some(stop)) = (start, stop) {
                if start <= stop {
                    is_scheduled_paused = now < start || now >= stop;
                } else {
                    is_scheduled_paused = now < start && now >= stop;
                }
            }
        }

        let mut downloading = db
            .get_downloads(&DownloadFilter {
                status: Some(DownloadStatus::Downloading),
                ..Default::default()
            })
            .map_err(|e| e.to_string())?;
        if let Ok(merging) = db.get_downloads(&DownloadFilter {
            status: Some(DownloadStatus::Merging),
            ..Default::default()
        }) {
            downloading.extend(merging);
        }

        if is_scheduled_paused {
            for mut dl in downloading {
                if let Some(gid) = &dl.aria2_gid {
                    let _ = engine.pause(gid).await;
                } else {
                    self.cancel_stream(dl.id.unwrap());
                }
                // Keep as Queued so schedule window reopens automatically
                dl.status = DownloadStatus::Queued;
                let _ = db.update_download(dl.id.unwrap(), &dl);
            }
            return Ok(());
        }

        let mut active_count = downloading.len();
        let max_concurrent_val = self.max_concurrent.load(Ordering::SeqCst);

        if active_count > max_concurrent_val {
            let mut sorted = downloading.clone();
            sorted.sort_by_key(|b| std::cmp::Reverse(b.priority));

            for mut dl in sorted.into_iter().skip(max_concurrent_val) {
                if let Some(gid) = &dl.aria2_gid {
                    let _ = engine.pause(gid).await;
                } else {
                    self.cancel_stream(dl.id.unwrap());
                }
                dl.status = DownloadStatus::Queued;
                let _ = db.update_download(dl.id.unwrap(), &dl);
                active_count = active_count.saturating_sub(1);
            }
        }

        if active_count < max_concurrent_val {
            if let Ok(mut queued) = db.get_downloads(&DownloadFilter {
                status: Some(DownloadStatus::Queued),
                ..Default::default()
            }) {
                queued.sort_by_key(|b| std::cmp::Reverse(b.priority));
                let max_conn = self.max_connections.load(Ordering::SeqCst) as u32;

                for mut dl in queued.into_iter().take(max_concurrent_val - active_count) {
                    let dl_id = match dl.id {
                        Some(id) => id,
                        None => continue,
                    };

                    // YouTube CDN links 403 outside browser — rewrite to watch URL + yt-dlp
                    if let Some(watch) =
                        youtube_page_url_for_download(&dl.url, dl.referrer.as_deref())
                    {
                        let (clean_url, ytdlp_fmt) = split_falcon_format(&watch);
                        if clean_url != dl.url {
                            dl.url = clean_url.clone();
                            let _ = db.update_download(dl_id, &dl);
                        } else if watch != dl.url {
                            let (c, _) = split_falcon_format(&watch);
                            dl.url = c;
                            let _ = db.update_download(dl_id, &dl);
                        }

                        let (tx, rx) = tokio::sync::watch::channel(false);
                        {
                            let mut map = lock_or_recover(&self.active_stream_tasks);
                            if map.contains_key(&dl_id) {
                                continue;
                            }
                            map.insert(dl_id, tx);
                        }

                        dl.status = DownloadStatus::Downloading;
                        if db.update_download(dl_id, &dl).is_err() {
                            lock_or_recover(&self.active_stream_tasks).remove(&dl_id);
                            continue;
                        }

                        let url = {
                            let (c, _) = split_falcon_format(&dl.url);
                            c
                        };
                        let save_path = dl.save_path.clone();
                        let mut final_path = std::path::PathBuf::from(&save_path);
                        // Force mp4 container after merge
                        let mut fname = dl.filename.clone();
                        if !fname.to_lowercase().ends_with(".mp4") {
                            if let Some(stem) = std::path::Path::new(&fname).file_stem() {
                                fname = format!("{}.mp4", stem.to_string_lossy());
                                dl.filename = fname.clone();
                                let _ = db.update_download(dl_id, &dl);
                            }
                        }
                        final_path.push(&fname);
                        let save_path_str = final_path.to_string_lossy().to_string();
                        let ytdlp_headers = crate::download::ytdlp::YtDlpHeaders {
                            cookies: dl.cookies.clone(),
                            user_agent: dl.user_agent.clone(),
                        };

                        let db_clone = db.clone();
                        let stream_tasks_clone = self.active_stream_tasks.clone();
                        let app_handle_clone = app_handle.clone();

                        tokio::spawn(async move {
                            // ponytail: shared finalize logic — see run_stream_task.
                            run_stream_task(
                                &app_handle_clone,
                                &db_clone,
                                &stream_tasks_clone,
                                dl_id,
                                &save_path_str,
                                StreamKind::YtDlp {
                                    url,
                                    headers: ytdlp_headers,
                                    format: ytdlp_fmt,
                                },
                                rx,
                            )
                            .await;
                        });
                        continue;
                    }

                    if is_hls_url(&dl.url) {
                        let (tx, rx) = tokio::sync::watch::channel(false);
                        {
                            let mut map = lock_or_recover(&self.active_stream_tasks);
                            if map.contains_key(&dl_id) {
                                continue;
                            }
                            map.insert(dl_id, tx);
                        }

                        // Claim in DB before spawn
                        dl.status = DownloadStatus::Downloading;
                        if db.update_download(dl_id, &dl).is_err() {
                            lock_or_recover(&self.active_stream_tasks).remove(&dl_id);
                            continue;
                        }

                        let url = dl.url.clone();
                        let save_path = dl.save_path.clone();
                        let mut final_path = std::path::PathBuf::from(&save_path);
                        final_path.push(&dl.filename);
                        let save_path_str = final_path.to_string_lossy().to_string();
                        let hls_headers = HlsHeaders {
                            cookies: dl.cookies.clone(),
                            referrer: dl.referrer.clone(),
                            user_agent: dl.user_agent.clone(),
                        };

                        let db_clone = db.clone();
                        let stream_tasks_clone = self.active_stream_tasks.clone();
                        let app_handle_clone = app_handle.clone();

                        tokio::spawn(async move {
                            // ponytail: shared finalize logic — see run_stream_task.
                            run_stream_task(
                                &app_handle_clone,
                                &db_clone,
                                &stream_tasks_clone,
                                dl_id,
                                &save_path_str,
                                StreamKind::Hls { url, headers: hls_headers },
                                rx,
                            )
                            .await;
                        });
                        continue; // already updated DB
                    } else if let Some(gid) = &dl.aria2_gid {
                        match engine.resume(gid).await {
                            Ok(()) => dl.status = DownloadStatus::Downloading,
                            Err(e) => {
                                log::warn!("aria2 resume failed, will re-add: {}", e);
                                dl.aria2_gid = None;
                                dl.status = DownloadStatus::Queued;
                            }
                        }
                    } else {
                        // Ensure aria2 is healthy before failing the download
                        if !engine.is_running() {
                            let dir = crate::util::app_data_dir();
                            if let Err(e) = engine.ensure_running(&app_handle, &dir).await {
                                log::warn!("aria2 ensure failed, keep queued: {}", e);
                                // Keep Queued so next tick retries — don't burn as Failed
                                continue;
                            }
                        }

                        let (split, max_connections) = self.conn_limits(
                            &dl.url,
                            max_conn,
                            dl.cookies.as_deref(),
                            dl.referrer.as_deref(),
                            dl.user_agent.as_deref(),
                        );
                        let mut headers = vec![];
                        if let Some(c) = &dl.cookies {
                            if !c.is_empty() {
                                headers.push(format!("Cookie: {}", sanitize_header_value(c)));
                            }
                        }
                        let aria_opts = Aria2Options {
                            dir: dl.save_path.clone(),
                            filename: dl.filename.clone(),
                            split,
                            max_connections,
                            headers,
                            referrer: dl.referrer.as_ref().map(|r| sanitize_header_value(r)),
                            user_agent: dl.user_agent.as_ref().map(|u| sanitize_header_value(u)),
                        };
                        match engine.add_download(&dl.url, aria_opts).await {
                            Ok(gid) => {
                                dl.aria2_gid = Some(gid);
                                dl.status = DownloadStatus::Downloading;
                                dl.error_message = None;
                            }
                            Err(e) => {
                                let msg = e.to_string();
                                log::error!("Aria2 engine add_download error: {}", msg);
                                // Transient engine/RPC → stay queued and retry
                                if msg.contains("not running")
                                    || msg.contains("Unauthorized")
                                    || msg.contains("Network error")
                                    || msg.contains("did not become ready")
                                {
                                    engine.mark_not_running();
                                    continue;
                                }
                                dl.status = DownloadStatus::Failed;
                                dl.error_message = Some(msg);
                            }
                        }
                    }
                    let _ = db.update_download(dl_id, &dl);
                }
            }
        }

        Ok(())
    }
}

/// What kind of streaming task to spawn. The spawn + finalize logic is identical
/// between yt-dlp and HLS; only the actual processor call differs. `save_path_str`
/// is passed separately to `run_stream_task` since both kinds need it for the
/// shared finalize (metadata read).
enum StreamKind {
    YtDlp { url: String, headers: crate::download::ytdlp::YtDlpHeaders, format: Option<String> },
    Hls { url: String, headers: HlsHeaders },
}

/// ponytail: shared completion/cleanup logic for yt-dlp and HLS tasks. Previously
/// this ~60-line block (Ok→Completed, Err→Paused/Failed, progress emit, map
/// cleanup) was copy-pasted for each processor. Centralizing it means a finalize
/// fix or status-emission change applies to both at once.
async fn run_stream_task(
    app_handle: &tauri::AppHandle,
    db: &Database,
    active_stream_tasks: &Arc<Mutex<HashMap<i64, tokio::sync::watch::Sender<bool>>>>,
    dl_id: i64,
    save_path_str: &str,
    kind: StreamKind,
    rx: tokio::sync::watch::Receiver<bool>,
) {
    let result = match kind {
        StreamKind::YtDlp { url, headers, format } => {
            crate::download::ytdlp::process_ytdlp(
                app_handle,
                dl_id,
                &url,
                save_path_str,
                rx,
                headers,
                Some(db.clone()),
                format.as_deref(),
            )
            .await
        }
        StreamKind::Hls { url, headers } => {
            crate::download::hls::process_hls_stream(
                app_handle,
                dl_id,
                &url,
                save_path_str,
                rx,
                headers,
                Some(db.clone()),
            )
            .await
        }
    };

    match result {
        Ok(_) => {
            let final_size =
                tokio::fs::metadata(&save_path_str).await.map(|m| m.len()).unwrap_or(0);
            if let Ok(mut d) = db.get_download(dl_id) {
                d.downloaded_size = final_size;
                d.total_size = final_size;
                d.speed = 0.0;
                d.status = DownloadStatus::Completed;
                d.completed_at = Some(chrono::Utc::now().to_rfc3339());
                d.error_message = None;
                // Cookie TTL: wipe session cookies after completion (at-rest hygiene).
                d.cookies = None;
                let _ = db.update_download(dl_id, &d);
            }
            let _ = app_handle.emit(
                "download-progress",
                serde_json::json!({
                    "id": dl_id,
                    "downloaded_size": final_size,
                    "total_size": final_size,
                    "speed": 0.0,
                    "status": "Completed",
                    "connections": 0
                }),
            );
        }
        Err(e) => {
            if e == "Cancelled" {
                let _ = db.update_download_progress(dl_id, 0, 0.0, &DownloadStatus::Paused);
            } else {
                log::error!("stream task {dl_id} error: {e}");
                // ponytail: classify transient vs fatal. Transient errors (rate
                // limits, temporary unavailability, network blips) flip the job
                // back to Queued so the next tick retries it instead of burning
                // it as permanently Failed — matching the aria2 transient path.
                let lower = e.to_lowercase();
                let is_transient = lower.contains("temporarily")
                    || lower.contains("rate limit")
                    || lower.contains("too many requests")
                    || lower.contains("try again")
                    || lower.contains("timed out")
                    || lower.contains("timeout")
                    || lower.contains("connection reset")
                    || lower.contains("network error");
                if let Ok(mut d) = db.get_download(dl_id) {
                    d.error_message = Some(e);
                    d.speed = 0.0;
                    d.status =
                        if is_transient { DownloadStatus::Queued } else { DownloadStatus::Failed };
                    let _ = db.update_download(dl_id, &d);
                }
            }
        }
    }
    lock_or_recover(active_stream_tasks).remove(&dl_id);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_queue_manager_new() {
        let qm = QueueManager::new();
        assert_eq!(qm.max_concurrent.load(Ordering::SeqCst), 3);
        assert!(!qm.get_schedule().active);
    }

    #[test]
    fn test_cancel_stream_missing() {
        let qm = QueueManager::new();
        assert!(!qm.cancel_stream(999));
    }

    #[test]
    fn test_head_probe_cache_hit_and_ttl() {
        let qm = QueueManager::new();
        // miss on empty cache
        assert!(qm.cached_conn_limit("https://x.com/a").is_none());
        // fresh hit
        qm.head_probe_cache.lock().unwrap().insert("https://x.com/a".into(), (8, Instant::now()));
        assert_eq!(qm.cached_conn_limit("https://x.com/a"), Some(8));
        // expired (> 5 min TTL)
        let stale = Instant::now().checked_sub(Duration::from_secs(301)).expect("system uptime");
        qm.head_probe_cache.lock().unwrap().insert("https://y.com/b".into(), (4, stale));
        assert!(qm.cached_conn_limit("https://y.com/b").is_none());
    }

    // ponytail: the probe cache must be bounded — previously it grew forever
    // (a slow memory leak). Verify eviction trims to HEAD_CACHE_CAP and drops
    // expired entries.
    #[test]
    fn test_head_probe_cache_eviction_bounds_size() {
        let qm = QueueManager::new();
        // Fill beyond the cap with fresh entries.
        for i in 0..(QueueManager::HEAD_CACHE_CAP + 50) {
            qm.head_probe_cache
                .lock()
                .unwrap()
                .insert(format!("https://x.com/{i}"), (4, Instant::now()));
        }
        QueueManager::evict_stale_probe_entries(&qm.head_probe_cache);
        let len = qm.head_probe_cache.lock().unwrap().len();
        assert!(
            len <= QueueManager::HEAD_CACHE_CAP,
            "cache size {len} exceeded cap {}",
            QueueManager::HEAD_CACHE_CAP
        );
    }

    #[test]
    fn test_head_probe_cache_eviction_drops_expired() {
        let qm = QueueManager::new();
        let stale = Instant::now().checked_sub(Duration::from_secs(301)).expect("system uptime");
        qm.head_probe_cache.lock().unwrap().insert("https://stale.com/a".into(), (4, stale));
        qm.head_probe_cache
            .lock()
            .unwrap()
            .insert("https://fresh.com/b".into(), (8, Instant::now()));
        QueueManager::evict_stale_probe_entries(&qm.head_probe_cache);
        let cache = qm.head_probe_cache.lock().unwrap();
        assert!(!cache.contains_key("https://stale.com/a"));
        assert!(cache.contains_key("https://fresh.com/b"));
    }
}
