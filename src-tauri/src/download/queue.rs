use crate::download::engine::{Aria2Engine, Aria2Options};
use crate::storage::{
    models::{DownloadFilter, DownloadStatus},
    Database,
};
use chrono::{Local, NaiveTime};
use serde::{Deserialize, Serialize};
use std::sync::{Arc, Mutex};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ScheduleOptions {
    pub start_time: Option<String>, // "HH:MM"
    pub stop_time: Option<String>,  // "HH:MM"
    pub active: bool,
}

use std::collections::HashMap;
use std::sync::atomic::{AtomicUsize, Ordering};

async fn determine_connection_limits(url: &str) -> (u32, u32) {
    if url.starts_with("magnet:") || url.ends_with(".torrent") {
        return (16, 16); // Torrents can use max connections
    }

    let client = reqwest::Client::new();
    if let Ok(res) = client.head(url).send().await {
        if let Some(len) = res.content_length() {
            if len < 10 * 1024 * 1024 {
                return (2, 2);
            }
            if len < 100 * 1024 * 1024 {
                return (8, 8);
            }
            return (16, 16);
        }
    }
    (4, 4) // Default fallback
}

pub struct QueueManager {
    pub schedule: Arc<Mutex<ScheduleOptions>>,
    pub max_concurrent: Arc<AtomicUsize>,
    pub active_hls_tasks: Arc<Mutex<HashMap<i64, tokio::sync::watch::Sender<bool>>>>,
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
            active_hls_tasks: Arc::new(Mutex::new(HashMap::new())),
        }
    }

    pub fn set_schedule(&self, opts: ScheduleOptions) {
        let mut lock = self.schedule.lock().unwrap();
        *lock = opts;
    }

    pub fn get_schedule(&self) -> ScheduleOptions {
        self.schedule.lock().unwrap().clone()
    }

    pub fn set_concurrent_downloads(&self, limit: usize) {
        self.max_concurrent.store(limit, Ordering::SeqCst);
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

            let start = opts
                .start_time
                .as_ref()
                .and_then(|s| NaiveTime::parse_from_str(s, "%H:%M").ok());
            let stop = opts
                .stop_time
                .as_ref()
                .and_then(|s| NaiveTime::parse_from_str(s, "%H:%M").ok());

            if let (Some(start), Some(stop)) = (start, stop) {
                if start <= stop {
                    is_scheduled_paused = now < start || now >= stop;
                } else {
                    is_scheduled_paused = now < start && now >= stop;
                }
            }
        }

        let downloading = db
            .get_downloads(&DownloadFilter {
                status: Some(DownloadStatus::Downloading),
                ..Default::default()
            })
            .map_err(|e| e.to_string())?;

        if is_scheduled_paused {
            for mut dl in downloading {
                if let Some(gid) = &dl.aria2_gid {
                    let _ = engine.pause(gid).await;
                }
                dl.status = DownloadStatus::Paused; // Wait, actually keep it as queued or paused? Let's use Paused or a new Scheduled status. We'll use Paused.
                let _ = db.update_download(dl.id.unwrap(), &dl);
            }
            return Ok(());
        }

        let mut active_count = downloading.len();

        // Enforce max concurrent downloads by pausing extras
        let max_concurrent_val = self.max_concurrent.load(Ordering::SeqCst);
        if active_count > max_concurrent_val {
            let mut sorted = downloading.clone();
            // Sort by priority descending, then created_at
            sorted.sort_by_key(|b| std::cmp::Reverse(b.priority));

            for mut dl in sorted.into_iter().skip(max_concurrent_val) {
                if let Some(gid) = &dl.aria2_gid {
                    let _ = engine.pause(gid).await;
                } else if dl.url.contains(".m3u8") {
                    // Send cancellation signal to HLS task
                    if let Some(tx) = self
                        .active_hls_tasks
                        .lock()
                        .unwrap()
                        .remove(&dl.id.unwrap())
                    {
                        let _ = tx.send(true);
                    }
                }
                dl.status = DownloadStatus::Queued;
                let _ = db.update_download(dl.id.unwrap(), &dl);
                active_count -= 1;
            }
        }

        if active_count < max_concurrent_val {
            if let Ok(mut queued) = db.get_downloads(&DownloadFilter {
                status: Some(DownloadStatus::Queued),
                ..Default::default()
            }) {
                queued.sort_by_key(|b| std::cmp::Reverse(b.priority));

                for mut dl in queued.into_iter().take(max_concurrent_val - active_count) {
                    if dl.url.contains(".m3u8") {
                        // Handle HLS
                        let dl_id = dl.id.unwrap();
                        let url = dl.url.clone();
                        let save_path = dl.save_path.clone();
                        let mut final_path = std::path::PathBuf::from(&save_path);
                        final_path.push(&dl.filename);
                        let save_path_str = final_path.to_string_lossy().to_string();

                        let db_clone = db.clone();
                        let (tx, rx) = tokio::sync::watch::channel(false);
                        self.active_hls_tasks.lock().unwrap().insert(dl_id, tx);

                        let hls_tasks_clone = self.active_hls_tasks.clone();
                        let app_handle_clone = app_handle.clone();

                        tokio::spawn(async move {
                            // Update status to Merging for simplicity during process (or Downloading)
                            let _ = db_clone.update_download_progress(
                                dl_id,
                                0,
                                0.0,
                                &DownloadStatus::Downloading,
                            );

                            match crate::download::hls::process_hls_stream(
                                &app_handle_clone,
                                &url,
                                &save_path_str,
                                rx,
                            )
                            .await
                            {
                                Ok(_) => {
                                    let _ = db_clone.update_download_progress(
                                        dl_id,
                                        100,
                                        0.0,
                                        &DownloadStatus::Completed,
                                    );
                                }
                                Err(e) => {
                                    if e == "Cancelled" {
                                        let _ = db_clone.update_download_progress(
                                            dl_id,
                                            0,
                                            0.0,
                                            &DownloadStatus::Paused,
                                        );
                                    } else {
                                        println!("HLS Error: {}", e);
                                        let _ = db_clone.update_download_progress(
                                            dl_id,
                                            0,
                                            0.0,
                                            &DownloadStatus::Failed,
                                        );
                                    }
                                }
                            }
                            // Cleanup task tracking
                            hls_tasks_clone.lock().unwrap().remove(&dl_id);
                        });
                        dl.status = DownloadStatus::Downloading;
                    } else if let Some(gid) = &dl.aria2_gid {
                        let _ = engine.resume(gid).await;
                        dl.status = DownloadStatus::Downloading;
                    } else {
                        // Needs to be added to aria2
                        let (split, max_connections) = determine_connection_limits(&dl.url).await;
                        let aria_opts = Aria2Options {
                            dir: dl.save_path.clone(),
                            filename: dl.filename.clone(),
                            split,
                            max_connections,
                            headers: vec![],
                            referrer: None,
                            user_agent: None,
                        };
                        if let Ok(gid) = engine.add_download(&dl.url, aria_opts).await {
                            dl.aria2_gid = Some(gid);
                        }
                        dl.status = DownloadStatus::Downloading;
                    }
                    let _ = db.update_download(dl.id.unwrap(), &dl);
                }
            }
        }

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_queue_manager_new() {
        let qm = QueueManager::new();
        assert_eq!(qm.max_concurrent.load(Ordering::SeqCst), 3);
        assert_eq!(qm.get_schedule().active, false);
    }
}
