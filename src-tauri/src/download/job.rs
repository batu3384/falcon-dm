//! Per-download context handed to every worker (HTTP, HLS, DASH, YouTube).

use crate::download::limiter::CompositeRateLimiter;
#[cfg(test)]
use crate::download::limiter::RateLimiter;
use crate::download::progress::Progress;
use crate::download::DownloadError;
use crate::storage::{models::DownloadCategory, Database};
use crate::util::{
    move_file_unique, sanitize_filename, unique_download_filename_reserved,
    validate_completed_file_named_with_hints, FinalizeHints,
};
use std::collections::HashSet;
use std::path::{Path, PathBuf};
#[cfg(test)]
use std::sync::Arc;
use tauri::{AppHandle, Emitter};
use tokio::sync::watch;

#[derive(Clone)]
pub struct JobContext {
    pub id: i64,
    pub app: Option<AppHandle>,
    pub db: Option<Database>,
    pub cancel: watch::Receiver<bool>,
    pub limiter: CompositeRateLimiter,
    pub progress: Progress,
}

impl JobContext {
    pub fn new(
        id: i64,
        app: Option<AppHandle>,
        db: Option<Database>,
        cancel: watch::Receiver<bool>,
        limiter: CompositeRateLimiter,
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
        self.finalize_with_hints(temp, dest, FinalizeHints::default()).await
    }

    /// `finalize` with the response facts needed to distinguish deliberate
    /// empty or small binary files from error pages.
    pub async fn finalize_with_hints(
        &self,
        temp: &Path,
        dest: &Path,
        hints: FinalizeHints,
    ) -> Result<PathBuf, DownloadError> {
        self.check_cancel()?;
        let reserved = dest.parent().map(|dir| self.reserved_names(dir)).unwrap_or_default();
        // The expectation lives on the row, not in the job setup, so every
        // engine (HTTP, HLS, YouTube) is covered without queue plumbing. Read
        // it fresh: nothing about verification depends on the worker's start.
        let expected_hash = self
            .db
            .as_ref()
            .and_then(|db| db.get_download(self.id).ok())
            .and_then(|row| row.expected_hash);
        let temp_owned = temp.to_path_buf();
        let dest_owned = dest.to_path_buf();
        let id = self.id;
        let landed = tokio::task::spawn_blocking(move || -> Result<PathBuf, String> {
            // A rejected temp can never be resumed: its resume state is only
            // meaningful next to the partial bytes, so both are dropped or a
            // retry would trip over an orphan.
            let drop_temp_and_state = || {
                let _ = std::fs::remove_file(&temp_owned);
                if let Some(state) = crate::download::http::hidden_sibling(&dest_owned, id, "state")
                {
                    let _ = std::fs::remove_file(state);
                }
            };
            if let Err(reason) =
                validate_completed_file_named_with_hints(&temp_owned, &dest_owned, hints)
            {
                drop_temp_and_state();
                return Err(reason);
            }
            // Verify before the move: bytes that fail the check never reach
            // the target folder. The temp is deleted because corrupt segments
            // can neither be trusted nor incrementally repaired — a retry
            // must re-download cleanly.
            if let Some(expected) = expected_hash {
                if let Err(reason) =
                    crate::download::checksum::verify_file_hash(&temp_owned, &expected)
                {
                    drop_temp_and_state();
                    return Err(reason);
                }
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
        (
            Self::new(
                id,
                None,
                None,
                rx,
                CompositeRateLimiter::global_only(Arc::new(RateLimiter::new(0))),
            ),
            tx,
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::util::FinalizeHints;

    fn scratch(tag: &str) -> PathBuf {
        let root = std::env::temp_dir().join(format!("falcon-job-{tag}-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&root).unwrap();
        root
    }

    async fn finalize_payload(
        root: &Path,
        filename: &str,
        payload: &[u8],
        hints: FinalizeHints,
    ) -> Result<PathBuf, DownloadError> {
        let temp = root.join(format!(".{filename}.71.falcon.part"));
        std::fs::write(&temp, payload).unwrap();
        let (ctx, _cancel) = JobContext::for_tests(71);
        ctx.finalize_with_hints(&temp, &root.join(filename), hints).await
    }

    #[tokio::test]
    async fn finalize_accepts_empty_file_when_content_length_was_zero() {
        let root = scratch("empty-cl0");
        let landed = finalize_payload(
            &root,
            "empty.bin",
            b"",
            FinalizeHints { expected_size: Some(0), ..Default::default() },
        )
        .await
        .unwrap();
        assert_eq!(landed.file_name().unwrap(), "empty.bin");
        assert_eq!(std::fs::metadata(&landed).unwrap().len(), 0);
        assert!(!root.join(".empty.bin.71.falcon.part").exists());
        std::fs::remove_dir_all(root).unwrap();
    }

    #[tokio::test]
    async fn finalize_accepts_small_binary_starting_with_lt_when_size_matches() {
        let root = scratch("small-binary");
        let payload = b"<\0\xff\x00valid-binary";
        let landed = finalize_payload(
            &root,
            "firmware.bin",
            payload,
            FinalizeHints { expected_size: Some(payload.len() as u64), ..Default::default() },
        )
        .await
        .unwrap();
        assert_eq!(std::fs::read(&landed).unwrap(), payload);
        std::fs::remove_dir_all(root).unwrap();
    }

    #[tokio::test]
    async fn finalize_rejects_html_error_page_when_response_was_html() {
        let root = scratch("html-error");
        let payload = b"<html>403</html>";
        let result = finalize_payload(
            &root,
            "archive.zip",
            payload,
            FinalizeHints { expected_size: Some(payload.len() as u64), response_is_html: true },
        )
        .await;
        assert_eq!(
            result,
            Err(DownloadError::Fatal("Server returned an error page instead of a file".into()))
        );
        assert!(!root.join(".archive.zip.71.falcon.part").exists(), "bad temp must be deleted");
        assert!(!root.join("archive.zip").exists());
        std::fs::remove_dir_all(root).unwrap();
    }

    #[tokio::test]
    async fn finalize_rejects_error_page_when_server_promised_a_larger_file() {
        let root = scratch("size-mismatch");
        let result = finalize_payload(
            &root,
            "archive.zip",
            b"{\"error\":\"blocked\"}",
            FinalizeHints { expected_size: Some(100_000), ..Default::default() },
        )
        .await;
        assert_eq!(
            result,
            Err(DownloadError::Fatal("Server returned an error page instead of a file".into()))
        );
        std::fs::remove_dir_all(root).unwrap();
    }

    /// Row carrying `expected_hash`, plus the JobContext a real worker would get.
    fn hashed_job(
        root: &Path,
        filename: &str,
        expected_hash: Option<&str>,
    ) -> (crate::storage::Database, JobContext, i64) {
        use crate::storage::models::{Download, DownloadStatus};
        let db = crate::storage::Database::in_memory().unwrap();
        let id = db
            .insert_download(&Download {
                id: None,
                url: "https://example.com/x".into(),
                filename: filename.into(),
                save_path: root.to_str().unwrap().into(),
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
                expected_hash: expected_hash.map(str::to_string),
                speed_limit_kbps: None,
                subtitle_tracks: None,
            })
            .unwrap();
        let (_cancel, rx) = watch::channel(false);
        let ctx = JobContext::new(
            id,
            None,
            Some(db.clone()),
            rx,
            CompositeRateLimiter::global_only(Arc::new(RateLimiter::new(0))),
        );
        (db, ctx, id)
    }

    fn write_temp(root: &Path, filename: &str, id: i64, payload: &[u8]) -> PathBuf {
        let temp = root.join(format!(".{filename}.{id}.falcon.part"));
        std::fs::write(&temp, payload).unwrap();
        temp
    }

    #[tokio::test]
    async fn finalize_fails_and_deletes_temp_when_checksum_mismatches() {
        let root = scratch("sum-mismatch");
        let payload = b"the actual file bytes";
        // sha256 of different bytes (the empty string).
        let wrong = "sha256:e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855";
        let (_db, ctx, id) = hashed_job(&root, "file.bin", Some(wrong));
        let temp = write_temp(&root, "file.bin", id, payload);
        let state = root.join(format!(".file.bin.{id}.falcon.state"));
        std::fs::write(&state, b"{}").unwrap();
        let result = ctx.finalize(&temp, &root.join("file.bin")).await;
        assert_eq!(result, Err(DownloadError::Fatal("ERR_CHECKSUM_MISMATCH".into())));
        assert!(!temp.exists(), "untrusted bytes must not survive as a partial file");
        assert!(!state.exists(), "the orphaned state must not survive either");
        assert!(!root.join("file.bin").exists(), "nothing may land in the target folder");
        std::fs::remove_dir_all(root).unwrap();
    }

    #[tokio::test]
    async fn finalize_lands_file_when_sha256_matches() {
        let root = scratch("sum-match");
        let payload = b"abc";
        let (db, ctx, id) = hashed_job(
            &root,
            "file.bin",
            Some("sha256:ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"),
        );
        let temp = write_temp(&root, "file.bin", id, payload);
        let landed = ctx.finalize(&temp, &root.join("file.bin")).await.unwrap();
        assert_eq!(std::fs::read(&landed).unwrap(), payload);
        drop(db);
        std::fs::remove_dir_all(root).unwrap();
    }

    #[tokio::test]
    async fn finalize_lands_file_when_md5_matches() {
        let root = scratch("sum-md5");
        let payload = b"abc";
        let (_db, ctx, id) =
            hashed_job(&root, "file.bin", Some("md5:900150983cd24fb0d6963f7d28e17f72"));
        let temp = write_temp(&root, "file.bin", id, payload);
        let landed = ctx.finalize(&temp, &root.join("file.bin")).await.unwrap();
        assert_eq!(std::fs::read(&landed).unwrap(), payload);
        std::fs::remove_dir_all(root).unwrap();
    }

    #[tokio::test]
    async fn finalize_without_a_hash_behaves_as_before() {
        let root = scratch("sum-none");
        let payload = b"anything at all";
        let (_db, ctx, id) = hashed_job(&root, "file.bin", None);
        let temp = write_temp(&root, "file.bin", id, payload);
        let landed = ctx.finalize(&temp, &root.join("file.bin")).await.unwrap();
        assert_eq!(std::fs::read(&landed).unwrap(), payload);
        std::fs::remove_dir_all(root).unwrap();
    }
}
