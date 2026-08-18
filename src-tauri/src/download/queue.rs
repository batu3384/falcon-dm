use crate::download::engine::Aria2Engine;
use crate::download::hls::HlsHeaders;
use crate::download::http::{HttpHeaders, HttpOptions};
use crate::storage::{
    models::{DownloadFilter, DownloadStatus},
    Database,
};
use crate::util::{
    is_hls_url, lock_or_recover, split_falcon_format, youtube_page_url_for_download,
};
use chrono::{Local, NaiveTime};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use tauri::Emitter;

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

#[derive(Debug, PartialEq, Eq)]
pub(crate) enum DownloadRoute {
    YtDlp,
    Hls,
    Http,
    Aria2,
}

pub(crate) fn route_queued_download(
    url: &str,
    referrer: Option<&str>,
    has_aria2_gid: bool,
) -> DownloadRoute {
    if youtube_page_url_for_download(url, referrer).is_some() {
        DownloadRoute::YtDlp
    } else if is_hls_url(url) {
        DownloadRoute::Hls
    } else if has_aria2_gid {
        DownloadRoute::Aria2
    } else {
        DownloadRoute::Http
    }
}

pub struct QueueManager {
    pub schedule: Arc<Mutex<ScheduleOptions>>,
    pub max_concurrent: Arc<AtomicUsize>,
    pub max_connections: Arc<AtomicUsize>,
    pub http_options: Arc<Mutex<HttpOptions>>,
    pub active_stream_tasks: Arc<Mutex<HashMap<i64, tokio::sync::watch::Sender<bool>>>>,
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
            http_options: Arc::new(Mutex::new(HttpOptions::default())),
            active_stream_tasks: Arc::new(Mutex::new(HashMap::new())),
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

    pub fn set_http_options(&self, proxy: Option<String>, speed_limit_kbps: u32) {
        *lock_or_recover(&self.http_options) = HttpOptions { proxy, speed_limit_kbps };
    }

    pub fn cancel_stream(&self, id: i64) -> bool {
        if let Some(tx) = lock_or_recover(&self.active_stream_tasks).get(&id) {
            let _ = tx.send(true);
            true
        } else {
            false
        }
    }

    pub fn is_stream_active(&self, id: i64) -> bool {
        lock_or_recover(&self.active_stream_tasks).contains_key(&id)
    }

    pub async fn cancel_and_wait_stream(&self, id: i64) -> bool {
        if !self.cancel_stream(id) {
            return true;
        }
        tokio::time::timeout(std::time::Duration::from_secs(35), async {
            while self.is_stream_active(id) {
                tokio::time::sleep(std::time::Duration::from_millis(50)).await;
            }
        })
        .await
        .is_ok()
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
                limit: Some(256),
                ..Default::default()
            })
            .map_err(|e| e.to_string())?;
        if let Ok(merging) = db.get_downloads(&DownloadFilter {
            status: Some(DownloadStatus::Merging),
            limit: Some(256),
            ..Default::default()
        }) {
            downloading.extend(merging);
        }

        if is_scheduled_paused {
            for dl in downloading {
                if let Some(gid) = &dl.aria2_gid {
                    let _ = engine.pause(gid).await;
                } else {
                    if !self.cancel_and_wait_stream(dl.id.unwrap()).await {
                        log::warn!("scheduled pause timed out for download {}", dl.id.unwrap());
                        continue;
                    }
                }
                let _ = db.set_status_if_current(
                    dl.id.unwrap(),
                    &[DownloadStatus::Downloading, DownloadStatus::Merging, DownloadStatus::Paused],
                    &DownloadStatus::Queued,
                );
            }
            return Ok(());
        }

        let mut active_count = downloading.len();
        let max_concurrent_val = self.max_concurrent.load(Ordering::SeqCst);

        if active_count > max_concurrent_val {
            let mut sorted = downloading.clone();
            sorted.sort_by_key(|b| std::cmp::Reverse(b.priority));

            for dl in sorted.into_iter().skip(max_concurrent_val) {
                if let Some(gid) = &dl.aria2_gid {
                    let _ = engine.pause(gid).await;
                } else {
                    if !self.cancel_and_wait_stream(dl.id.unwrap()).await {
                        log::warn!("concurrency pause timed out for download {}", dl.id.unwrap());
                        continue;
                    }
                }
                let _ = db.set_status_if_current(
                    dl.id.unwrap(),
                    &[DownloadStatus::Downloading, DownloadStatus::Merging, DownloadStatus::Paused],
                    &DownloadStatus::Queued,
                );
                active_count = active_count.saturating_sub(1);
            }
        }

        if active_count < max_concurrent_val {
            if let Ok(queued) = db.take_queued((max_concurrent_val - active_count) as i64) {
                for mut dl in queued {
                    let dl_id = match dl.id {
                        Some(id) => id,
                        None => continue,
                    };

                    match route_queued_download(
                        &dl.url,
                        dl.referrer.as_deref(),
                        dl.aria2_gid.is_some(),
                    ) {
                        DownloadRoute::YtDlp => {
                            let Some(watch) =
                                youtube_page_url_for_download(&dl.url, dl.referrer.as_deref())
                            else {
                                continue;
                            };
                            let Some(rx) = self.try_claim_stream(db, dl_id) else {
                                continue;
                            };
                            dl.status = DownloadStatus::Downloading;
                            let (clean_url, ytdlp_fmt) = split_falcon_format(&watch);
                            let mut fname = dl.filename.clone();
                            if !fname.to_lowercase().ends_with(".mp4") {
                                if let Some(stem) = std::path::Path::new(&fname).file_stem() {
                                    fname = format!("{}.mp4", stem.to_string_lossy());
                                    let _ = db.set_filename_if_current(
                                        dl_id,
                                        &[DownloadStatus::Downloading],
                                        &fname,
                                    );
                                }
                            }
                            let mut final_path = std::path::PathBuf::from(&dl.save_path);
                            final_path.push(&fname);
                            self.spawn_stream(
                                db,
                                app_handle.clone(),
                                dl_id,
                                final_path.to_string_lossy().to_string(),
                                StreamKind::YtDlp {
                                    url: clean_url,
                                    headers: crate::download::ytdlp::YtDlpHeaders {
                                        cookies: dl.cookies.clone(),
                                        user_agent: dl.user_agent.clone(),
                                    },
                                    format: ytdlp_fmt,
                                },
                                rx,
                            );
                            continue;
                        }
                        DownloadRoute::Hls => {
                            let Some(rx) = self.try_claim_stream(db, dl_id) else {
                                continue;
                            };
                            dl.status = DownloadStatus::Downloading;
                            let mut final_path = std::path::PathBuf::from(&dl.save_path);
                            final_path.push(&dl.filename);
                            self.spawn_stream(
                                db,
                                app_handle.clone(),
                                dl_id,
                                final_path.to_string_lossy().to_string(),
                                StreamKind::Hls {
                                    url: dl.url.clone(),
                                    headers: HlsHeaders {
                                        cookies: dl.cookies.clone(),
                                        referrer: dl.referrer.clone(),
                                        user_agent: dl.user_agent.clone(),
                                        max_connections: self
                                            .max_connections
                                            .load(Ordering::SeqCst),
                                    },
                                },
                                rx,
                            );
                            continue;
                        }
                        DownloadRoute::Http => {
                            let Some(rx) = self.try_claim_stream(db, dl_id) else {
                                continue;
                            };
                            let mut final_path = std::path::PathBuf::from(&dl.save_path);
                            final_path.push(&dl.filename);
                            self.spawn_stream(
                                db,
                                app_handle.clone(),
                                dl_id,
                                final_path.to_string_lossy().to_string(),
                                StreamKind::Http {
                                    url: dl.url.clone(),
                                    headers: HttpHeaders {
                                        cookies: dl.cookies.clone(),
                                        referrer: dl.referrer.clone(),
                                        user_agent: dl.user_agent.clone(),
                                        options: lock_or_recover(&self.http_options).clone(),
                                        max_connections: self
                                            .max_connections
                                            .load(Ordering::SeqCst),
                                    },
                                },
                                rx,
                            );
                            continue;
                        }
                        DownloadRoute::Aria2 => {
                            let Some(gid) = &dl.aria2_gid else {
                                continue;
                            };
                            if engine
                                .ensure_running(&app_handle, &crate::util::app_data_dir())
                                .await
                                .is_err()
                            {
                                log::warn!("aria2 unavailable, switching {} to HTTP", dl_id);
                                let _ =
                                    db.clear_aria2_gid_if_current(dl_id, &[DownloadStatus::Queued]);
                                continue;
                            }
                            match engine.resume(gid).await {
                                Ok(()) => {
                                    if db
                                        .set_status_if_current(
                                            dl_id,
                                            &[DownloadStatus::Queued],
                                            &DownloadStatus::Downloading,
                                        )
                                        .unwrap_or(false)
                                    {
                                        dl.status = DownloadStatus::Downloading;
                                    } else {
                                        let _ = engine.pause(gid).await;
                                    }
                                }
                                Err(e) => {
                                    log::warn!("aria2 resume failed, switching to HTTP: {}", e);
                                    let _ = engine.remove(gid).await;
                                    let _ = db.clear_aria2_gid_if_current(
                                        dl_id,
                                        &[DownloadStatus::Queued],
                                    );
                                }
                            }
                        }
                    }
                }
            }
        }

        Ok(())
    }

    fn try_claim_stream(
        &self,
        db: &Database,
        dl_id: i64,
    ) -> Option<tokio::sync::watch::Receiver<bool>> {
        let (tx, rx) = tokio::sync::watch::channel(false);
        {
            let mut map = lock_or_recover(&self.active_stream_tasks);
            if map.contains_key(&dl_id) {
                return None;
            }
            map.insert(dl_id, tx);
        }
        if !db
            .set_status_error_if_current(
                dl_id,
                &[DownloadStatus::Queued],
                &DownloadStatus::Downloading,
                None,
                None,
            )
            .unwrap_or(false)
        {
            lock_or_recover(&self.active_stream_tasks).remove(&dl_id);
            return None;
        }
        Some(rx)
    }

    fn spawn_stream(
        &self,
        db: &Database,
        app_handle: tauri::AppHandle,
        dl_id: i64,
        save_path_str: String,
        kind: StreamKind,
        rx: tokio::sync::watch::Receiver<bool>,
    ) {
        let db_clone = db.clone();
        let stream_tasks_clone = self.active_stream_tasks.clone();
        tokio::spawn(async move {
            run_stream_task(
                &app_handle,
                &db_clone,
                &stream_tasks_clone,
                dl_id,
                &save_path_str,
                kind,
                rx,
            )
            .await;
        });
    }
}

/// What kind of streaming task to spawn. The spawn + finalize logic is identical
/// between yt-dlp and HLS; only the actual processor call differs. `save_path_str`
/// is passed separately to `run_stream_task` since both kinds need it for the
/// shared finalize (metadata read).
enum StreamKind {
    YtDlp { url: String, headers: crate::download::ytdlp::YtDlpHeaders, format: Option<String> },
    Hls { url: String, headers: HlsHeaders },
    Http { url: String, headers: HttpHeaders },
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
        StreamKind::Http { url, headers } => {
            crate::download::http::process_http(
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
            if db.finish_stream_if_active(dl_id, final_size).unwrap_or(false) {
                let _ = db.clear_session_cookies(dl_id);
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
            } else {
                // The row may have been deleted while the worker was finishing.
                // Never remove a destination that the user may still own.
            }
        }
        Err(e) => {
            if e == "Cancelled" {
                let _ = db.pause_stream_if_active(dl_id);
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
                let next_status =
                    if is_transient { DownloadStatus::Queued } else { DownloadStatus::Failed };
                if db
                    .set_status_error_if_current(
                        dl_id,
                        &[DownloadStatus::Downloading, DownloadStatus::Merging],
                        &next_status,
                        Some(&e),
                        Some(0.0),
                    )
                    .unwrap_or(false)
                    && !is_transient
                {
                    let _ = db.clear_session_cookies(dl_id);
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
    fn cancel_stream_keeps_active_claim_until_worker_cleanup() {
        let qm = QueueManager::new();
        let (tx, rx) = tokio::sync::watch::channel(false);
        qm.active_stream_tasks.lock().unwrap().insert(7, tx);

        assert!(qm.cancel_stream(7));
        assert!(qm.active_stream_tasks.lock().unwrap().contains_key(&7));
        assert!(*rx.borrow());
    }

    #[tokio::test]
    async fn cancel_and_wait_stream_returns_after_worker_cleanup() {
        let qm = QueueManager::new();
        let (tx, _rx) = tokio::sync::watch::channel(false);
        qm.active_stream_tasks.lock().unwrap().insert(7, tx);
        let active = qm.active_stream_tasks.clone();
        tokio::spawn(async move {
            tokio::time::sleep(std::time::Duration::from_millis(5)).await;
            active.lock().unwrap().remove(&7);
        });

        assert!(qm.cancel_and_wait_stream(7).await);
        assert!(!qm.is_stream_active(7));
    }

    #[test]
    fn overnight_schedule_is_valid_but_equal_or_malformed_is_not() {
        assert!(validate_schedule(&ScheduleOptions {
            start_time: Some("23:00".into()),
            stop_time: Some("06:00".into()),
            active: true,
        })
        .is_ok());
        assert!(validate_schedule(&ScheduleOptions {
            start_time: Some("06:00".into()),
            stop_time: Some("06:00".into()),
            active: true,
        })
        .is_err());
        assert!(validate_schedule(&ScheduleOptions {
            start_time: Some("6:00".into()),
            stop_time: Some("07:00".into()),
            active: true,
        })
        .is_err());
        assert!(validate_schedule(&ScheduleOptions {
            start_time: Some("24:00".into()),
            stop_time: Some("07:00".into()),
            active: true,
        })
        .is_err());
    }

    #[test]
    fn route_uses_path_not_query_for_hls() {
        assert_eq!(
            route_queued_download("https://cdn.example.com/video.mp4?x=.m3u8", None, false),
            DownloadRoute::Http
        );
        assert_eq!(
            route_queued_download("https://cdn.example.com/live.m3u8", None, false),
            DownloadRoute::Hls
        );
        assert_eq!(
            route_queued_download("https://www.youtube.com/watch?v=abc", None, false),
            DownloadRoute::YtDlp
        );
        assert_eq!(
            route_queued_download("https://cdn.example.com/a.bin", None, true),
            DownloadRoute::Aria2
        );
    }
}
