use crate::storage::{Database, models::{DownloadStatus, DownloadFilter}};
use crate::download::engine::{Aria2Engine, Aria2Options};
use chrono::{Local, NaiveTime};
use std::sync::{Arc, Mutex};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ScheduleOptions {
    pub start_time: Option<String>, // "HH:MM"
    pub stop_time: Option<String>,  // "HH:MM"
    pub active: bool,
}

pub struct QueueManager {
    pub schedule: Arc<Mutex<ScheduleOptions>>,
    pub max_concurrent: usize,
}

impl QueueManager {
    pub fn new() -> Self {
        Self {
            schedule: Arc::new(Mutex::new(ScheduleOptions {
                start_time: None,
                stop_time: None,
                active: false,
            })),
            max_concurrent: 3,
        }
    }

    pub fn set_schedule(&self, opts: ScheduleOptions) {
        let mut lock = self.schedule.lock().unwrap();
        *lock = opts;
    }

    pub fn get_schedule(&self) -> ScheduleOptions {
        self.schedule.lock().unwrap().clone()
    }

    pub async fn tick(&self, db: &Database, engine: &Aria2Engine) -> Result<(), String> {
        let opts = self.get_schedule();
        let mut is_scheduled_paused = false;

        if opts.active {
            let now = Local::now().time();
            
            let start = opts.start_time.as_ref().and_then(|s| NaiveTime::parse_from_str(s, "%H:%M").ok());
            let stop = opts.stop_time.as_ref().and_then(|s| NaiveTime::parse_from_str(s, "%H:%M").ok());

            if let (Some(start), Some(stop)) = (start, stop) {
                if start <= stop {
                    is_scheduled_paused = now < start || now >= stop;
                } else {
                    is_scheduled_paused = now < start && now >= stop;
                }
            }
        }

        let downloading = db.get_downloads(&DownloadFilter {
            status: Some(DownloadStatus::Downloading),
            ..Default::default()
        }).map_err(|e| e.to_string())?;

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
        if active_count > self.max_concurrent {
            let mut sorted = downloading.clone();
            // Sort by priority descending, then created_at
            sorted.sort_by(|a, b| b.priority.cmp(&a.priority));
            
            for mut dl in sorted.into_iter().skip(self.max_concurrent) {
                if let Some(gid) = &dl.aria2_gid {
                    let _ = engine.pause(gid).await;
                }
                dl.status = DownloadStatus::Queued;
                let _ = db.update_download(dl.id.unwrap(), &dl);
                active_count -= 1;
            }
        }

        // If we have room, start queued downloads
        if active_count < self.max_concurrent {
            if let Ok(mut queued) = db.get_downloads(&DownloadFilter {
                status: Some(DownloadStatus::Queued),
                ..Default::default()
            }) {
                queued.sort_by(|a, b| b.priority.cmp(&a.priority));

                for mut dl in queued.into_iter().take(self.max_concurrent - active_count) {
                    if let Some(gid) = &dl.aria2_gid {
                        let _ = engine.resume(gid).await;
                    } else {
                        // Needs to be added to aria2
                        let aria_opts = Aria2Options {
                            dir: dl.save_path.clone(),
                            filename: dl.filename.clone(),
                            split: 16,
                            max_connections: 16,
                            headers: vec![],
                            referrer: None,
                            user_agent: None,
                        };
                        if let Ok(gid) = engine.add_download(&dl.url, aria_opts).await {
                            dl.aria2_gid = Some(gid);
                        }
                    }
                    dl.status = DownloadStatus::Downloading;
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
        assert_eq!(qm.max_concurrent, 3);
        assert_eq!(qm.get_schedule().active, false);
    }
}
