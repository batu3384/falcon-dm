use super::database::{Database, DatabaseError, Result};
use super::models::{Download, DownloadCategory, DownloadFilter, DownloadStatus};
use crate::util::{open_cookies, seal_cookies, youtube_dedup_base};
use chrono::Utc;
use rusqlite::OptionalExtension;
use rusqlite::{params, Row, TransactionBehavior};
use std::collections::HashSet;

/// Column order `row_to_download` expects.
const DOWNLOAD_COLUMNS: &str = "id, url, filename, save_path, total_size, downloaded_size, \
     status, category, speed, segments, priority, created_at, completed_at, error_message, \
     referrer, user_agent, cookies, archived, attempts, retry_at, expected_hash, \
     speed_limit_kbps";

fn stored_speed_limit(limit: Option<u32>) -> Option<i64> {
    limit.filter(|kbps| *kbps > 0).map(i64::from)
}

fn like_escape(value: &str) -> String {
    value.replace('\\', "\\\\").replace('%', "\\%").replace('_', "\\_")
}

fn like_contains_pattern(search: &str) -> String {
    format!("%{}%", like_escape(search.trim()))
}

pub enum InsertDownloadResult {
    Created(i64),
    Existing(i64),
}

impl InsertDownloadResult {
    pub fn id(&self) -> i64 {
        match self {
            Self::Created(id) | Self::Existing(id) => *id,
        }
    }
}

#[derive(Debug, Default, Clone)]
pub struct DownloadStatsData {
    pub active: u64,
    pub queued: u64,
    pub paused: u64,
    pub completed: u64,
    pub failed: u64,
    pub total_downloaded_bytes: u64,
    pub current_speed: f64,
    pub all: u64,
    pub archived: u64,
    pub video: u64,
    pub music: u64,
    pub document: u64,
    pub archive: u64,
    pub program: u64,
}

impl Database {
    pub fn insert_download(&self, download: &Download) -> Result<i64> {
        let conn = self.conn.get().map_err(|e| DatabaseError::PoolError(e.to_string()))?;
        conn.execute(
            "INSERT INTO downloads (
                url, filename, save_path, total_size, downloaded_size, status, category,
                speed, segments, priority, created_at, completed_at, error_message,
                referrer, user_agent, cookies, archived, expected_hash, speed_limit_kbps
            ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16, ?17, ?18, ?19)",
            params![
                download.url,
                download.filename,
                download.save_path,
                download.total_size as i64,
                download.downloaded_size as i64,
                download.status.as_str(),
                download.category.as_str(),
                download.speed,
                download.segments as i64,
                download.priority as i64,
                download.created_at,
                download.completed_at,
                download.error_message,
                download.referrer,
                download.user_agent,
                seal_cookies(download.cookies.as_deref()),
                download.archived as i64,
                download.expected_hash,
                stored_speed_limit(download.speed_limit_kbps),
            ],
        )?;
        Ok(conn.last_insert_rowid())
    }

    pub fn insert_download_deduped(&self, download: &mut Download) -> Result<InsertDownloadResult> {
        // The dedupe comparison is an exact string match, so the stored URL
        // must be canonical: case/port/spelling variants of one target collapse.
        download.url = crate::canonical_store_url(&download.url);
        let mut conn = self.conn.get().map_err(|e| DatabaseError::PoolError(e.to_string()))?;
        let tx = conn
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(DatabaseError::from)?;
        let existing = if let Some(base) = youtube_dedup_base(&download.url) {
            // `_`/`%` in watch IDs are LIKE wildcards unless escaped (F-86).
            let fmt_pattern = format!("{}#falconfmt=%", like_escape(&base));
            tx.query_row(
                "SELECT id FROM downloads
                 WHERE (url = ?1 OR url = ?2 OR url LIKE ?3 ESCAPE '\\')
                   AND status IN ('Queued', 'Downloading', 'Paused', 'Merging')
                   AND archived = 0
                 ORDER BY id DESC
                 LIMIT 1",
                params![download.url, base, fmt_pattern],
                |row| row.get::<_, i64>(0),
            )
            .optional()?
        } else {
            tx.query_row(
                "SELECT id FROM downloads
                 WHERE url = ?1
                   AND status IN ('Queued', 'Downloading', 'Paused', 'Merging')
                   AND archived = 0
                 ORDER BY id DESC
                 LIMIT 1",
                params![download.url],
                |row| row.get::<_, i64>(0),
            )
            .optional()?
        };
        if let Some(id) = existing {
            tx.commit().map_err(DatabaseError::from)?;
            return Ok(InsertDownloadResult::Existing(id));
        }
        let mut reserved = HashSet::new();
        {
            let mut stmt =
                tx.prepare("SELECT filename FROM downloads WHERE save_path = ?1 AND archived = 0")?;
            let rows =
                stmt.query_map(params![&download.save_path], |row| row.get::<_, String>(0))?;
            for name in rows {
                reserved.insert(name?);
            }
        }
        download.filename = crate::util::unique_download_filename_reserved(
            &download.save_path,
            &download.filename,
            &reserved,
        )
        .map_err(DatabaseError::PoolError)?;
        tx.execute(
            "INSERT INTO downloads (
                url, filename, save_path, total_size, downloaded_size, status, category,
                speed, segments, priority, created_at, completed_at, error_message,
                referrer, user_agent, cookies, archived, expected_hash, speed_limit_kbps
            ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16, ?17, ?18, ?19)",
            params![
                download.url,
                download.filename,
                download.save_path,
                download.total_size as i64,
                download.downloaded_size as i64,
                download.status.as_str(),
                download.category.as_str(),
                download.speed,
                download.segments as i64,
                download.priority as i64,
                download.created_at,
                download.completed_at,
                download.error_message,
                download.referrer,
                download.user_agent,
                seal_cookies(download.cookies.as_deref()),
                download.archived as i64,
                download.expected_hash,
                stored_speed_limit(download.speed_limit_kbps),
            ],
        )?;
        let id = tx.last_insert_rowid();
        tx.commit().map_err(DatabaseError::from)?;
        Ok(InsertDownloadResult::Created(id))
    }

    pub fn reserved_filenames(
        &self,
        save_path: &str,
        except_id: Option<i64>,
    ) -> Result<HashSet<String>> {
        let conn = self.conn.get().map_err(|e| DatabaseError::PoolError(e.to_string()))?;
        let mut names = HashSet::new();
        if let Some(id) = except_id {
            let mut stmt = conn.prepare(
                "SELECT filename FROM downloads WHERE save_path = ?1 AND archived = 0 AND id != ?2",
            )?;
            let rows = stmt.query_map(params![save_path, id], |row| row.get::<_, String>(0))?;
            for name in rows {
                names.insert(name?);
            }
        } else {
            let mut stmt = conn
                .prepare("SELECT filename FROM downloads WHERE save_path = ?1 AND archived = 0")?;
            let rows = stmt.query_map(params![save_path], |row| row.get::<_, String>(0))?;
            for name in rows {
                names.insert(name?);
            }
        }
        Ok(names)
    }

    pub fn update_download(&self, id: i64, download: &Download) -> Result<()> {
        let conn = self.conn.get().map_err(|e| DatabaseError::PoolError(e.to_string()))?;
        let rows = conn.execute(
            "UPDATE downloads SET
                url = ?1,
                filename = ?2,
                save_path = ?3,
                total_size = ?4,
                downloaded_size = ?5,
                status = ?6,
                category = ?7,
                speed = ?8,
                segments = ?9,
                priority = ?10,
                created_at = ?11,
                completed_at = ?12,
                error_message = ?13,
                referrer = ?14,
                user_agent = ?15,
                cookies = ?16,
                archived = ?17,
                expected_hash = ?18,
                speed_limit_kbps = ?19
            WHERE id = ?20",
            params![
                download.url,
                download.filename,
                download.save_path,
                download.total_size as i64,
                download.downloaded_size as i64,
                download.status.as_str(),
                download.category.as_str(),
                download.speed,
                download.segments as i64,
                download.priority as i64,
                download.created_at,
                download.completed_at,
                download.error_message,
                download.referrer,
                download.user_agent,
                seal_cookies(download.cookies.as_deref()),
                download.archived as i64,
                download.expected_hash,
                stored_speed_limit(download.speed_limit_kbps),
                id,
            ],
        )?;
        if rows == 0 {
            return Err(DatabaseError::NotFound(id));
        }
        Ok(())
    }

    pub fn set_status_if_current(
        &self,
        id: i64,
        expected: &[DownloadStatus],
        status: &DownloadStatus,
    ) -> Result<bool> {
        if expected.is_empty() {
            return Ok(false);
        }
        let conn = self.conn.get().map_err(|e| DatabaseError::PoolError(e.to_string()))?;
        let placeholders =
            (0..expected.len()).map(|index| format!("?{}", index + 3)).collect::<Vec<_>>();
        let sql = format!(
            "UPDATE downloads SET status = ?1
             WHERE id = ?2 AND status IN ({})",
            placeholders.join(", ")
        );
        let mut params: Vec<Box<dyn rusqlite::ToSql>> =
            vec![Box::new(status.as_str().to_string()), Box::new(id)];
        params.extend(
            expected
                .iter()
                .map(|value| Box::new(value.as_str().to_string()) as Box<dyn rusqlite::ToSql>),
        );
        let params_ref: Vec<&dyn rusqlite::ToSql> =
            params.iter().map(|value| value.as_ref()).collect();
        Ok(conn.execute(&sql, params_ref.as_slice())? == 1)
    }

    pub fn set_status_error_if_current(
        &self,
        id: i64,
        expected: &[DownloadStatus],
        status: &DownloadStatus,
        error_message: Option<&str>,
        speed: Option<f64>,
    ) -> Result<bool> {
        if expected.is_empty() {
            return Ok(false);
        }
        let conn = self.conn.get().map_err(|e| DatabaseError::PoolError(e.to_string()))?;
        let placeholders =
            (0..expected.len()).map(|index| format!("?{}", index + 5)).collect::<Vec<_>>();
        let sql = format!(
            "UPDATE downloads SET status = ?1, error_message = ?2, speed = COALESCE(?3, speed)
             WHERE id = ?4 AND status IN ({})",
            placeholders.join(", ")
        );
        let mut params: Vec<Box<dyn rusqlite::ToSql>> = vec![
            Box::new(status.as_str().to_string()),
            Box::new(error_message.map(str::to_string)),
            Box::new(speed),
            Box::new(id),
        ];
        params.extend(
            expected
                .iter()
                .map(|value| Box::new(value.as_str().to_string()) as Box<dyn rusqlite::ToSql>),
        );
        let params_ref: Vec<&dyn rusqlite::ToSql> =
            params.iter().map(|value| value.as_ref()).collect();
        Ok(conn.execute(&sql, params_ref.as_slice())? == 1)
    }

    pub fn resume_if_current(&self, id: i64, expected: &[DownloadStatus]) -> Result<bool> {
        if expected.is_empty() {
            return Ok(false);
        }
        let conn = self.conn.get().map_err(|e| DatabaseError::PoolError(e.to_string()))?;
        let placeholders =
            (0..expected.len()).map(|index| format!("?{}", index + 2)).collect::<Vec<_>>();
        let sql = format!(
            "UPDATE downloads
             SET status = 'Queued', priority = priority + 1, attempts = 0, retry_at = NULL,
                 completed_at = NULL, error_message = NULL, speed = 0.0
             WHERE id = ?1 AND status IN ({})",
            placeholders.join(", ")
        );
        let mut params: Vec<Box<dyn rusqlite::ToSql>> = vec![Box::new(id)];
        params.extend(
            expected
                .iter()
                .map(|value| Box::new(value.as_str().to_string()) as Box<dyn rusqlite::ToSql>),
        );
        let params_ref: Vec<&dyn rusqlite::ToSql> =
            params.iter().map(|value| value.as_ref()).collect();
        Ok(conn.execute(&sql, params_ref.as_slice())? == 1)
    }

    pub fn set_filename_if_current(
        &self,
        id: i64,
        expected: &[DownloadStatus],
        filename: &str,
    ) -> Result<bool> {
        if expected.is_empty() {
            return Ok(false);
        }
        let conn = self.conn.get().map_err(|e| DatabaseError::PoolError(e.to_string()))?;
        let placeholders =
            (0..expected.len()).map(|index| format!("?{}", index + 3)).collect::<Vec<_>>();
        let sql = format!(
            "UPDATE downloads SET filename = ?1
             WHERE id = ?2 AND status IN ({})",
            placeholders.join(", ")
        );
        let mut params: Vec<Box<dyn rusqlite::ToSql>> =
            vec![Box::new(filename.to_string()), Box::new(id)];
        params.extend(
            expected
                .iter()
                .map(|value| Box::new(value.as_str().to_string()) as Box<dyn rusqlite::ToSql>),
        );
        let params_ref: Vec<&dyn rusqlite::ToSql> =
            params.iter().map(|value| value.as_ref()).collect();
        Ok(conn.execute(&sql, params_ref.as_slice())? == 1)
    }

    pub fn set_archived_if_status(
        &self,
        id: i64,
        archived: bool,
        allowed: &[DownloadStatus],
    ) -> Result<bool> {
        if allowed.is_empty() {
            return Ok(false);
        }
        let conn = self.conn.get().map_err(|e| DatabaseError::PoolError(e.to_string()))?;
        let placeholders =
            (0..allowed.len()).map(|index| format!("?{}", index + 3)).collect::<Vec<_>>();
        let sql = format!(
            "UPDATE downloads SET archived = ?1
             WHERE id = ?2 AND status IN ({})",
            placeholders.join(", ")
        );
        let mut params: Vec<Box<dyn rusqlite::ToSql>> =
            vec![Box::new(archived as i64), Box::new(id)];
        params.extend(
            allowed
                .iter()
                .map(|value| Box::new(value.as_str().to_string()) as Box<dyn rusqlite::ToSql>),
        );
        let params_ref: Vec<&dyn rusqlite::ToSql> =
            params.iter().map(|value| value.as_ref()).collect();
        Ok(conn.execute(&sql, params_ref.as_slice())? == 1)
    }

    pub fn adjust_priority(&self, id: i64, increase: bool) -> Result<bool> {
        let conn = self.conn.get().map_err(|e| DatabaseError::PoolError(e.to_string()))?;
        let rows = conn.execute(
            "UPDATE downloads
             SET priority = CASE
                 WHEN ?1 = 1 THEN MIN(priority + 1, 4294967295)
                 ELSE MAX(priority - 1, 0)
             END
             WHERE id = ?2",
            params![increase as i64, id],
        )?;
        Ok(rows == 1)
    }

    /// Per-download cap in decimal KB/s. Zero is stored as NULL: the row
    /// follows the global bucket. Accepted in every status so the inspector can
    /// prepare a stopped row or adjust a live one.
    pub fn set_speed_limit(&self, id: i64, kbps: u32) -> Result<bool> {
        let conn = self.conn.get().map_err(|e| DatabaseError::PoolError(e.to_string()))?;
        let rows = conn.execute(
            "UPDATE downloads SET speed_limit_kbps = ?1 WHERE id = ?2",
            params![stored_speed_limit(Some(kbps)), id],
        )?;
        Ok(rows == 1)
    }

    /// Park a failed attempt back in the queue. `retry_at` (unix ms) keeps
    /// `take_queued` from restarting it before the backoff elapses.
    pub fn schedule_retry(
        &self,
        id: i64,
        expected: &[DownloadStatus],
        attempts: u32,
        retry_at_ms: i64,
        error_message: &str,
    ) -> Result<bool> {
        if expected.is_empty() {
            return Ok(false);
        }
        let conn = self.conn.get().map_err(|e| DatabaseError::PoolError(e.to_string()))?;
        let placeholders =
            (0..expected.len()).map(|index| format!("?{}", index + 5)).collect::<Vec<_>>();
        let sql = format!(
            "UPDATE downloads
             SET status = 'Queued', attempts = ?1, retry_at = ?2, error_message = ?3, speed = 0.0
             WHERE id = ?4 AND status IN ({})",
            placeholders.join(", ")
        );
        let mut params: Vec<Box<dyn rusqlite::ToSql>> = vec![
            Box::new(i64::from(attempts)),
            Box::new(retry_at_ms),
            Box::new(error_message.to_string()),
            Box::new(id),
        ];
        params.extend(
            expected
                .iter()
                .map(|value| Box::new(value.as_str().to_string()) as Box<dyn rusqlite::ToSql>),
        );
        let params_ref: Vec<&dyn rusqlite::ToSql> =
            params.iter().map(|value| value.as_ref()).collect();
        Ok(conn.execute(&sql, params_ref.as_slice())? == 1)
    }

    /// "Resume" on a row waiting out a retry backoff: start it on the next tick.
    pub fn clear_retry_backoff(&self, id: i64) -> Result<bool> {
        let conn = self.conn.get().map_err(|e| DatabaseError::PoolError(e.to_string()))?;
        let rows = conn.execute(
            "UPDATE downloads SET retry_at = NULL, attempts = 0
             WHERE id = ?1 AND status = 'Queued' AND retry_at IS NOT NULL",
            params![id],
        )?;
        Ok(rows == 1)
    }

    /// Swap the link of a stopped download (expired signatures are the common
    /// case) and clear the retry state. A Failed row requeues; a Paused row
    /// stays paused because the user chose to pause it. `keep_cookies = false`
    /// drops the session: the caller recomputes whether the new origin may
    /// receive the old one's cookies. The `WHERE` clause is the race guard: a
    /// row that started running since the caller looked does not match, so a
    /// live worker never has its URL swapped mid-flight.
    pub fn refresh_url_if_stopped(&self, id: i64, url: &str, keep_cookies: bool) -> Result<bool> {
        let conn = self.conn.get().map_err(|e| DatabaseError::PoolError(e.to_string()))?;
        let rows = conn.execute(
            "UPDATE downloads
             SET url = ?1,
                 status = CASE WHEN status = 'Failed' THEN 'Queued' ELSE status END,
                 error_message = NULL, attempts = 0, retry_at = NULL, speed = 0.0,
                 cookies = CASE WHEN ?3 THEN cookies ELSE NULL END
             WHERE id = ?2 AND status IN ('Failed', 'Paused')",
            params![url, id, keep_cookies],
        )?;
        Ok(rows == 1)
    }

    /// A worker that received real bytes is healthy again: forget earlier failures.
    pub fn clear_retry_state(&self, id: i64) -> Result<bool> {
        let conn = self.conn.get().map_err(|e| DatabaseError::PoolError(e.to_string()))?;
        let rows = conn.execute(
            "UPDATE downloads SET attempts = 0, retry_at = NULL, error_message = NULL
             WHERE id = ?1 AND status IN ('Downloading', 'Merging')
               AND (attempts != 0 OR retry_at IS NOT NULL OR error_message IS NOT NULL)",
            params![id],
        )?;
        Ok(rows == 1)
    }

    /// The server (Content-Disposition) or the muxer may decide the real name
    /// after the row exists. Category follows the new extension.
    pub fn rename_active_download(
        &self,
        id: i64,
        filename: &str,
        category: &DownloadCategory,
    ) -> Result<bool> {
        let conn = self.conn.get().map_err(|e| DatabaseError::PoolError(e.to_string()))?;
        let rows = conn.execute(
            "UPDATE downloads SET filename = ?1, category = ?2
             WHERE id = ?3 AND status IN ('Queued', 'Downloading', 'Merging', 'Paused')",
            params![filename, category.as_str(), id],
        )?;
        Ok(rows == 1)
    }

    /// Completes a worker that validated a full file. Includes Paused: user or
    /// tray pause may land before a slow finalize returns Ok. Cancelled workers
    /// never call this, so a real pause stays Paused. Failed stays Failed.
    pub fn finish_stream_if_active(&self, id: i64, size: u64) -> Result<bool> {
        let conn = self.conn.get().map_err(|e| DatabaseError::PoolError(e.to_string()))?;
        let rows = conn.execute(
            "UPDATE downloads
             SET downloaded_size = ?1,
                 total_size = ?1,
                 speed = 0.0,
                 status = 'Completed',
                 completed_at = ?2,
                 error_message = NULL,
                 attempts = 0,
                 retry_at = NULL
             WHERE id = ?3 AND status IN ('Downloading', 'Merging', 'Queued', 'Paused')",
            params![size as i64, Utc::now().to_rfc3339(), id],
        )?;
        Ok(rows == 1)
    }

    pub fn clear_session_cookies(&self, id: i64) -> Result<bool> {
        let conn = self.conn.get().map_err(|e| DatabaseError::PoolError(e.to_string()))?;
        let rows = conn.execute(
            "UPDATE downloads SET cookies = NULL WHERE id = ?1 AND cookies IS NOT NULL",
            params![id],
        )?;
        Ok(rows == 1)
    }

    /// Upgrade leftover plaintext cookie rows after a version bump (R-03).
    pub fn reseal_legacy_cookies(&self) -> Result<()> {
        let conn = self.conn.get().map_err(|e| DatabaseError::PoolError(e.to_string()))?;
        let mut stmt =
            conn.prepare("SELECT id, cookies FROM downloads WHERE cookies IS NOT NULL")?;
        let rows = stmt
            .query_map([], |row| Ok((row.get::<_, i64>(0)?, row.get::<_, Option<String>>(1)?)))?;
        let mut pending = Vec::new();
        for row in rows {
            let (id, stored) = row?;
            if crate::util::cookies_are_sealed(stored.as_deref()) {
                continue;
            }
            if let Some(sealed) = seal_cookies(stored.as_deref()) {
                pending.push((id, sealed));
            }
        }
        drop(stmt);
        for (id, sealed) in pending {
            conn.execute("UPDATE downloads SET cookies = ?1 WHERE id = ?2", params![sealed, id])?;
        }
        Ok(())
    }

    /// Written by the single progress reporter of a live download, which always
    /// holds the true counts (they may shrink when a transfer restarts).
    pub fn update_download_progress(
        &self,
        id: i64,
        downloaded_size: u64,
        total_size: Option<u64>,
        speed: f64,
        status: &DownloadStatus,
    ) -> Result<bool> {
        let conn = self.conn.get().map_err(|e| DatabaseError::PoolError(e.to_string()))?;
        let rows = conn.execute(
            "UPDATE downloads SET
             downloaded_size = ?1, total_size = COALESCE(?2, total_size), speed = ?3, status = ?4
             WHERE id = ?5 AND status IN ('Downloading', 'Merging')",
            params![
                downloaded_size as i64,
                total_size.map(|total| total as i64),
                speed,
                status.as_str(),
                id
            ],
        )?;
        Ok(rows == 1)
    }

    /// Next rows to start, skipping those still inside a retry backoff.
    pub fn take_queued(&self, limit: i64, now_ms: i64) -> Result<Vec<Download>> {
        let conn = self.conn.get().map_err(|e| DatabaseError::PoolError(e.to_string()))?;
        let mut stmt = conn.prepare(&format!(
            "SELECT {DOWNLOAD_COLUMNS} FROM downloads \
             WHERE status = 'Queued' AND (archived = 0 OR archived IS NULL) \
               AND (retry_at IS NULL OR retry_at <= ?1) \
             ORDER BY priority DESC, id ASC LIMIT ?2"
        ))?;
        let rows = stmt.query_map(params![now_ms, limit.max(0)], row_to_download)?;
        let mut downloads = Vec::new();
        for download in rows {
            downloads.push(download?);
        }
        Ok(downloads)
    }

    /// Rows the extension popup shows: everything live, then the newest finished ones.
    pub fn recent_activity(&self, limit: i64) -> Result<Vec<Download>> {
        let conn = self.conn.get().map_err(|e| DatabaseError::PoolError(e.to_string()))?;
        let mut stmt = conn.prepare(&format!(
            "SELECT {DOWNLOAD_COLUMNS} FROM downloads \
             WHERE (archived = 0 OR archived IS NULL) \
             ORDER BY CASE WHEN status IN ('Downloading', 'Merging') THEN 0 \
                           WHEN status = 'Queued' THEN 1 \
                           WHEN status = 'Paused' THEN 2 ELSE 3 END, id DESC LIMIT ?1"
        ))?;
        let rows = stmt.query_map(params![limit.clamp(1, 50)], row_to_download)?;
        let mut downloads = Vec::new();
        for download in rows {
            downloads.push(download?);
        }
        Ok(downloads)
    }

    pub fn download_stats(&self) -> Result<DownloadStatsData> {
        let conn = self.conn.get().map_err(|e| DatabaseError::PoolError(e.to_string()))?;
        let mut data = DownloadStatsData::default();
        let mut stmt = conn.prepare(
            "SELECT status, COUNT(*), COALESCE(SUM(downloaded_size), 0), \
                    COALESCE(SUM(CASE WHEN status IN ('Downloading', 'Merging') THEN speed ELSE 0 END), 0) \
             FROM downloads \
             WHERE (archived = 0 OR archived IS NULL) \
             GROUP BY status",
        )?;
        let mut rows = stmt.query([])?;
        while let Some(row) = rows.next()? {
            let status: String = row.get(0)?;
            let count: i64 = row.get(1)?;
            let bytes: i64 = row.get(2)?;
            let row_speed: f64 = row.get(3)?;
            let count = count.max(0) as u64;
            data.total_downloaded_bytes =
                data.total_downloaded_bytes.saturating_add(bytes.max(0) as u64);
            match status.as_str() {
                "Downloading" | "Merging" => {
                    data.active += count;
                    data.current_speed += row_speed;
                }
                "Queued" => data.queued += count,
                "Paused" => data.paused += count,
                "Completed" => data.completed += count,
                "Failed" => data.failed += count,
                _ => {}
            }
        }
        data.current_speed = data.current_speed.max(0.0);
        data.all = conn
            .query_row(
                "SELECT COUNT(*) FROM downloads WHERE archived = 0 OR archived IS NULL",
                [],
                |row| row.get::<_, i64>(0),
            )?
            .max(0) as u64;
        data.archived = conn
            .query_row("SELECT COUNT(*) FROM downloads WHERE archived = 1", [], |row| {
                row.get::<_, i64>(0)
            })?
            .max(0) as u64;
        let mut cat_stmt = conn.prepare(
            "SELECT category, COUNT(*) FROM downloads \
             WHERE (archived = 0 OR archived IS NULL) GROUP BY category",
        )?;
        let mut cat_rows = cat_stmt.query([])?;
        while let Some(row) = cat_rows.next()? {
            let category: String = row.get(0)?;
            let count = row.get::<_, i64>(1)?.max(0) as u64;
            match category.as_str() {
                "Video" => data.video += count,
                "Music" => data.music += count,
                "Document" => data.document += count,
                "Archive" => data.archive += count,
                "Program" => data.program += count,
                _ => {}
            }
        }
        Ok(data)
    }

    pub fn get_downloads(&self, filter: &DownloadFilter) -> Result<Vec<Download>> {
        let conn = self.conn.get().map_err(|e| DatabaseError::PoolError(e.to_string()))?;
        let mut sql = format!("SELECT {DOWNLOAD_COLUMNS} FROM downloads WHERE 1=1");

        let mut params: Vec<Box<dyn rusqlite::ToSql>> = Vec::new();

        if let Some(ref statuses) = filter.statuses {
            if statuses.is_empty() {
                sql.push_str(" AND 0");
            } else {
                sql.push_str(" AND status IN (");
                for (index, status) in statuses.iter().enumerate() {
                    if index > 0 {
                        sql.push(',');
                    }
                    sql.push('?');
                    params.push(Box::new(status.as_str().to_string()));
                }
                sql.push(')');
            }
        } else if let Some(ref status) = filter.status {
            sql.push_str(" AND status = ?");
            params.push(Box::new(status.as_str().to_string()));
        }

        if let Some(ref category) = filter.category {
            sql.push_str(" AND category = ?");
            params.push(Box::new(category.as_str().to_string()));
        }

        if let Some(ref search) = filter.search {
            if !search.trim().is_empty() {
                sql.push_str(" AND (filename LIKE ? ESCAPE '\\' OR url LIKE ? ESCAPE '\\')");
                let pattern = like_contains_pattern(search);
                params.push(Box::new(pattern.clone()));
                params.push(Box::new(pattern));
            }
        }

        if let Some(before_id) = filter.before_id {
            sql.push_str(" AND id < ?");
            params.push(Box::new(before_id));
        }

        // ponytail: archived filter. Default (None) hides archived rows so the
        // active list never shows them; the "Archived" view passes Some(true).
        match filter.archived {
            None => sql.push_str(" AND (archived = 0 OR archived IS NULL)"),
            Some(true) => sql.push_str(" AND archived = 1"),
            Some(false) => sql.push_str(" AND (archived = 0 OR archived IS NULL)"),
        }

        sql.push_str(" ORDER BY id DESC");

        // ponytail: negative LIMIT means "no limit" in SQLite; internal callers pass None → all rows.
        let limit = filter.limit.unwrap_or(-1);
        let offset = filter.offset.unwrap_or(0);
        sql.push_str(" LIMIT ? OFFSET ?");
        params.push(Box::new(limit));
        params.push(Box::new(offset));

        let params_ref: Vec<&dyn rusqlite::ToSql> = params.iter().map(|p| p.as_ref()).collect();
        let mut stmt = conn.prepare(&sql)?;
        let download_iter = stmt.query_map(params_ref.as_slice(), row_to_download)?;

        let mut downloads = Vec::new();
        for download in download_iter {
            downloads.push(download?);
        }

        Ok(downloads)
    }

    pub fn find_active_download_id_by_url(&self, url: &str) -> Result<Option<i64>> {
        let conn = self.conn.get().map_err(|e| DatabaseError::PoolError(e.to_string()))?;
        let result = conn.query_row(
            "SELECT id FROM downloads
             WHERE url = ?1
               AND status IN ('Queued', 'Downloading', 'Paused', 'Merging')
               AND (archived = 0 OR archived IS NULL)
             ORDER BY id DESC
             LIMIT 1",
            params![url],
            |row| row.get::<_, i64>(0),
        );
        match result {
            Ok(id) => Ok(Some(id)),
            Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
            Err(err) => Err(DatabaseError::from(err)),
        }
    }

    pub fn get_download(&self, id: i64) -> Result<Download> {
        let conn = self.conn.get().map_err(|e| DatabaseError::PoolError(e.to_string()))?;
        let mut stmt =
            conn.prepare(&format!("SELECT {DOWNLOAD_COLUMNS} FROM downloads WHERE id = ?1"))?;

        stmt.query_row(params![id], row_to_download).map_err(|err| match err {
            rusqlite::Error::QueryReturnedNoRows => DatabaseError::NotFound(id),
            other => DatabaseError::Sqlite(other),
        })
    }

    /// Introspect the SQLite journal mode (verify WAL is active).
    pub fn journal_mode(&self) -> Result<String> {
        let conn = self.conn.get().map_err(|e| DatabaseError::PoolError(e.to_string()))?;
        conn.query_row("PRAGMA journal_mode", [], |row| row.get::<_, String>(0))
            .map_err(DatabaseError::from)
    }

    pub fn delete_download(&self, id: i64) -> Result<()> {
        let conn = self.conn.get().map_err(|e| DatabaseError::PoolError(e.to_string()))?;
        let rows = conn.execute("DELETE FROM downloads WHERE id = ?1", params![id])?;
        if rows == 0 {
            return Err(DatabaseError::NotFound(id));
        }
        Ok(())
    }
}

fn row_to_download(row: &Row) -> rusqlite::Result<Download> {
    let id: i64 = row.get(0)?;
    let url: String = row.get(1)?;
    let filename: String = row.get(2)?;
    let save_path: String = row.get(3)?;
    let total_size: i64 = row.get(4)?;
    let downloaded_size: i64 = row.get(5)?;
    let status_str: String = row.get(6)?;
    let category_str: String = row.get(7)?;
    let speed: f64 = row.get(8)?;
    let segments: i64 = row.get(9)?;
    let priority: i64 = row.get(10)?;
    let created_at: String = row.get(11)?;
    let completed_at: Option<String> = row.get(12)?;
    let error_message: Option<String> = row.get(13)?;
    let referrer: Option<String> = row.get(14)?;
    let user_agent: Option<String> = row.get(15)?;
    let cookies: Option<String> = row.get(16)?;
    let archived: bool = row.get::<_, i64>(17)? != 0;
    let attempts: i64 = row.get(18)?;
    let retry_at: Option<i64> = row.get(19)?;
    let expected_hash: Option<String> = row.get(20)?;
    let speed_limit_kbps: Option<i64> = row.get(21)?;

    Ok(Download {
        id: Some(id),
        url,
        filename,
        save_path,
        total_size: total_size as u64,
        downloaded_size: downloaded_size as u64,
        status: status_str.parse().unwrap_or_else(|_| {
            log::warn!("downloads#{id}: unknown status '{status_str}', defaulting to Queued");
            DownloadStatus::Queued
        }),
        category: category_str.parse().unwrap_or_else(|_| {
            log::warn!("downloads#{id}: unknown category '{category_str}', defaulting to Other");
            DownloadCategory::Other
        }),
        speed,
        segments: segments as u32,
        priority: priority as u32,
        created_at,
        completed_at,
        error_message,
        referrer,
        user_agent,
        cookies: open_cookies(cookies.as_deref()),
        archived,
        attempts: attempts.clamp(0, i64::from(u32::MAX)) as u32,
        retry_at,
        expected_hash,
        speed_limit_kbps: speed_limit_kbps
            .and_then(|kbps| u32::try_from(kbps).ok())
            .filter(|kbps| *kbps > 0),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::storage::models::DownloadStatus;

    fn row(url: &str, name: &str, dir: &str) -> Download {
        Download {
            id: None,
            url: url.into(),
            filename: name.into(),
            save_path: dir.into(),
            total_size: 0,
            downloaded_size: 0,
            status: DownloadStatus::Queued,
            category: DownloadCategory::Other,
            speed: 0.0,
            segments: 2,
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
        }
    }

    #[test]
    fn insert_canonicalizes_url_and_dedupes_case_and_port_variants() {
        let db = Database::in_memory().unwrap();
        let dir = std::env::temp_dir().join(format!("falcon-canon-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let save = dir.to_str().unwrap();

        let mut first = row("HTTPS://CDN.EXAMPLE.COM:443/a.bin", "a.bin", save);
        let created = db.insert_download_deduped(&mut first).unwrap();
        assert_eq!(
            first.url, "https://cdn.example.com/a.bin",
            "the stored URL must be the canonical form"
        );

        for variant in [
            "https://cdn.example.com/a.bin",
            "HTTPS://cdn.example.com/a.bin",
            "https://CDN.EXAMPLE.COM/a.bin",
            "https://cdn.example.com:443/a.bin",
        ] {
            let mut again = row(variant, "other.bin", save);
            match db.insert_download_deduped(&mut again).unwrap() {
                InsertDownloadResult::Existing(id) => assert_eq!(id, created.id(), "{variant}"),
                InsertDownloadResult::Created(_) => {
                    panic!("{variant} must dedupe against the canonical row")
                }
            }
        }

        // Genuinely different URLs still create new rows.
        let mut other = row("https://cdn.example.com/b.bin", "b.bin", save);
        match db.insert_download_deduped(&mut other).unwrap() {
            InsertDownloadResult::Created(_) => {}
            InsertDownloadResult::Existing(_) => panic!("different file must not dedupe"),
        }
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn insert_canonicalization_keeps_signed_query_strings_verbatim() {
        let db = Database::in_memory().unwrap();
        let dir = std::env::temp_dir().join(format!("falcon-canon-q-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let save = dir.to_str().unwrap();
        let signed =
            "https://rr1---sn.example.com/videoplayback?itag=18&sig=abc%2Bd%2F%3D&range=0-100&sq=1";
        let mut first = row(signed, "v.bin", save);
        db.insert_download_deduped(&mut first).unwrap();
        assert_eq!(first.url, signed, "query bytes must survive canonicalization");
        // Non-HTTP(S) rows (blob uploads) pass through untouched.
        let mut blob = row("blob:https://example.com/uuid", "blob.bin", save);
        db.insert_download_deduped(&mut blob).unwrap();
        assert_eq!(blob.url, "blob:https://example.com/uuid");
        let _ = std::fs::remove_dir_all(dir);
    }

    /// A row that burned its retry budget: failed, backoff set, error recorded.
    fn failed_row(db: &Database, url: &str, name: &str, dir: &str) -> i64 {
        let mut download = row(url, name, dir);
        let id = match db.insert_download_deduped(&mut download).unwrap() {
            InsertDownloadResult::Created(id) => id,
            InsertDownloadResult::Existing(_) => panic!("fixture must create a row"),
        };
        assert!(db
            .schedule_retry(id, &[DownloadStatus::Queued], 3, 9_999_999, "HTTP 403")
            .unwrap());
        assert!(db
            .set_status_if_current(id, &[DownloadStatus::Queued], &DownloadStatus::Failed)
            .unwrap());
        id
    }

    #[test]
    fn refresh_url_requeues_failed_row_and_clears_retry_state() {
        let db = Database::in_memory().unwrap();
        let dir = std::env::temp_dir().join(format!("falcon-refresh-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let save = dir.to_str().unwrap();
        let id = failed_row(&db, "https://cdn.example.com/expired.bin", "f.bin", save);

        let fresh = "https://cdn.example.com/fresh.bin?sig=new";
        assert!(db.refresh_url_if_stopped(id, fresh, true).unwrap());
        let row = db.get_download(id).unwrap();
        assert_eq!(row.url, fresh);
        assert_eq!(row.status, DownloadStatus::Queued, "a failed row must requeue");
        assert_eq!(row.error_message, None);
        assert_eq!(row.attempts, 0);
        assert_eq!(row.retry_at, None);
        assert_eq!(row.speed, 0.0);
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn refresh_url_drops_cookies_only_when_told_to() {
        let db = Database::in_memory().unwrap();
        let dir = std::env::temp_dir().join(format!("falcon-refresh-c-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let save = dir.to_str().unwrap();

        let mut with_cookies = row("https://cdn.example.com/a.bin", "c.bin", save);
        with_cookies.cookies = Some("sid=secret".into());
        let id = match db.insert_download_deduped(&mut with_cookies).unwrap() {
            InsertDownloadResult::Created(id) => id,
            InsertDownloadResult::Existing(_) => panic!("fixture must create a row"),
        };
        assert!(db
            .set_status_if_current(id, &[DownloadStatus::Queued], &DownloadStatus::Failed)
            .unwrap());

        // Same-origin refresh keeps the session; cross-host drops it.
        assert!(db.refresh_url_if_stopped(id, "https://cdn.example.com/b.bin", true).unwrap());
        assert_eq!(db.get_download(id).unwrap().cookies.as_deref(), Some("sid=secret"));
        assert!(db
            .set_status_if_current(id, &[DownloadStatus::Queued], &DownloadStatus::Failed)
            .unwrap());
        assert!(db.refresh_url_if_stopped(id, "https://else.example.net/b.bin", false).unwrap());
        assert_eq!(db.get_download(id).unwrap().cookies, None);
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn refresh_url_keeps_paused_row_paused() {
        let db = Database::in_memory().unwrap();
        let dir = std::env::temp_dir().join(format!("falcon-refresh-p-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let save = dir.to_str().unwrap();
        let mut download = row("https://cdn.example.com/old.bin", "p.bin", save);
        let id = match db.insert_download_deduped(&mut download).unwrap() {
            InsertDownloadResult::Created(id) => id,
            InsertDownloadResult::Existing(_) => panic!("fixture must create a row"),
        };
        assert!(db
            .set_status_if_current(id, &[DownloadStatus::Queued], &DownloadStatus::Paused)
            .unwrap());

        assert!(db.refresh_url_if_stopped(id, "https://cdn.example.com/new.bin", true).unwrap());
        let row = db.get_download(id).unwrap();
        assert_eq!(row.url, "https://cdn.example.com/new.bin");
        assert_eq!(row.status, DownloadStatus::Paused, "a paused row stays user-paused");
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn refresh_url_rejects_live_queued_and_completed_rows() {
        let db = Database::in_memory().unwrap();
        let dir = std::env::temp_dir().join(format!("falcon-refresh-r-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let save = dir.to_str().unwrap();
        for (index, status) in [
            DownloadStatus::Downloading,
            DownloadStatus::Merging,
            DownloadStatus::Queued,
            DownloadStatus::Completed,
        ]
        .into_iter()
        .enumerate()
        {
            let mut download = row(
                &format!("https://cdn.example.com/{index}.bin"),
                &format!("r{index}.bin"),
                save,
            );
            let id = match db.insert_download_deduped(&mut download).unwrap() {
                InsertDownloadResult::Created(id) => id,
                InsertDownloadResult::Existing(_) => panic!("fixture must create a row"),
            };
            assert!(db.set_status_if_current(id, &[DownloadStatus::Queued], &status).unwrap());
            assert!(
                !db.refresh_url_if_stopped(id, "https://cdn.example.com/swapped.bin", true)
                    .unwrap(),
                "{status:?} must reject a url refresh"
            );
            assert_eq!(
                db.get_download(id).unwrap().url,
                format!("https://cdn.example.com/{index}.bin"),
                "{status:?} row url must be untouched"
            );
        }
        assert!(!db
            .refresh_url_if_stopped(999_999, "https://cdn.example.com/x.bin", true)
            .unwrap());
        let _ = std::fs::remove_dir_all(dir);
    }
}
