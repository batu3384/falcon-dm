//! Per-download context handed to every worker (HTTP, HLS, yt-dlp).

use crate::download::limiter::RateLimiter;
use crate::download::progress::Progress;
use crate::download::DownloadError;
use crate::storage::{models::DownloadCategory, Database};
use crate::util::{
    move_file_unique, sanitize_filename, unique_download_filename_reserved,
    validate_completed_file_named,
};
use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use tauri::{AppHandle, Emitter};
use tokio::sync::watch;

#[derive(Clone)]
pub struct JobContext {
    pub id: i64,
    pub app: Option<AppHandle>,
    pub db: Option<Database>,
    pub cancel: watch::Receiver<bool>,
    pub limiter: Arc<RateLimiter>,
    pub progress: Progress,
}

impl JobContext {
    pub fn new(
        id: i64,
        app: Option<AppHandle>,
        db: Option<Database>,
        cancel: watch::Receiver<bool>,
        limiter: Arc<RateLimiter>,
    ) -> Self {
        let progress = Progress::new(app.as_ref(), db.clone(), id);
        Self { id, app, db, cancel, limiter, progress }
    }

    pub fn is_cancelled(&self) -> bool {
        *self.cancel.borrow()
    }

    pub fn check_cancel(&self) -> Result<(), DownloadError> {
        if self.is_cancelled() {
            Err(DownloadError::Cancelled)
        } else {
            Ok(())
        }
    }

    fn reserved_names(&self, dir: &Path) -> HashSet<String> {
        let Some(db) = &self.db else {
            return HashSet::new();
        };
        dir.to_str()
            .and_then(|dir| db.reserved_filenames(dir, Some(self.id)).ok())
            .unwrap_or_default()
    }

    /// Tell the UI the row changed outside of progress (name, category).
    pub fn emit_row(&self) {
        let (Some(app), Some(db)) = (&self.app, &self.db) else {
            return;
        };
        if let Ok(row) = db.get_download(self.id) {
            let _ = app.emit("download-updated", &row);
        }
    }

    fn store_name(&self, name: &str) {
        if let Some(db) = &self.db {
            let category = DownloadCategory::from_filename(name);
            if db.rename_active_download(self.id, name, &category).unwrap_or(false) {
                self.emit_row();
            }
        }
    }

    /// Adopt a better filename before any bytes exist (server-provided name,
    /// container extension). Returns the destination the worker should use.
    pub fn rename(&self, dest: &Path, wanted: &str) -> PathBuf {
        let Some(dir) = dest.parent() else {
            return dest.to_path_buf();
        };
        let wanted = sanitize_filename(wanted);
        if dest.file_name().and_then(|n| n.to_str()) == Some(wanted.as_str()) {
            return dest.to_path_buf();
        }
        let reserved = self.reserved_names(dir);
        let name = dir
            .to_str()
            .and_then(|dir| unique_download_filename_reserved(dir, &wanted, &reserved).ok())
            .unwrap_or(wanted);
        self.store_name(&name);
        dir.join(name)
    }

    /// Validate `temp`, then move it into place without overwriting anything.
    /// If the name got taken meanwhile the file lands as `name (n).ext` and the
    /// row follows. An invalid temp file is deleted so a retry starts clean.
    pub async fn finalize(&self, temp: &Path, dest: &Path) -> Result<PathBuf, DownloadError> {
        self.check_cancel()?;
        let reserved = dest.parent().map(|dir| self.reserved_names(dir)).unwrap_or_default();
        let temp_owned = temp.to_path_buf();
        let dest_owned = dest.to_path_buf();
        let landed = tokio::task::spawn_blocking(move || -> Result<PathBuf, String> {
            if let Err(reason) = validate_completed_file_named(&temp_owned, &dest_owned) {
                let _ = std::fs::remove_file(&temp_owned);
                return Err(reason);
            }
            move_file_unique(&temp_owned, &dest_owned, &reserved)
        })
        .await
        .map_err(|e| DownloadError::fatal(format!("finalize task failed: {e}")))?
        .map_err(DownloadError::Fatal)?;
        // Compare names, not paths: `landed` is canonicalized (/var → /private/var).
        if landed.file_name() != dest.file_name() {
            if let Some(name) = landed.file_name().and_then(|n| n.to_str()) {
                self.store_name(name);
            }
        }
        Ok(landed)
    }
}

#[cfg(test)]
impl JobContext {
    /// Context without UI or DB, plus the sender that cancels it.
    pub fn for_tests(id: i64) -> (Self, watch::Sender<bool>) {
        let (tx, rx) = watch::channel(false);
        (Self::new(id, None, None, rx, Arc::new(RateLimiter::new(0))), tx)
    }
}
