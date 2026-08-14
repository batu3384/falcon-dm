use super::database::{Database, DatabaseError, Result};
use super::models::{Download, DownloadCategory, DownloadFilter, DownloadStatus};
use chrono::Utc;
use rusqlite::{params, Row};

fn like_contains_pattern(search: &str) -> String {
    let escaped = search.trim().replace('\\', "\\\\").replace('%', "\\%").replace('_', "\\_");
    format!("%{escaped}%")
}

impl Database {
    pub fn insert_download(&self, download: &Download) -> Result<i64> {
        let conn = self.conn.get().map_err(|e| DatabaseError::PoolError(e.to_string()))?;
        conn.execute(
            "INSERT INTO downloads (
                url, filename, save_path, total_size, downloaded_size, status, category,
                speed, segments, priority, created_at, completed_at, error_message,
                referrer, user_agent, cookies, aria2_gid, archived
            ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16, ?17, ?18)",
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
                download.cookies,
                download.aria2_gid,
                download.archived as i64,
            ],
        )?;
        Ok(conn.last_insert_rowid())
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
                aria2_gid = ?17,
                archived = ?18
            WHERE id = ?19",
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
                download.cookies,
                download.aria2_gid,
                download.archived as i64,
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

    pub fn update_progress_if_current(
        &self,
        id: i64,
        expected: &[DownloadStatus],
        downloaded_size: u64,
        total_size: u64,
        speed: f64,
        status: &DownloadStatus,
        completed_at: Option<&str>,
        error_message: Option<&str>,
    ) -> Result<bool> {
        if expected.is_empty() {
            return Ok(false);
        }
        let conn = self.conn.get().map_err(|e| DatabaseError::PoolError(e.to_string()))?;
        let placeholders =
            (0..expected.len()).map(|index| format!("?{}", index + 8)).collect::<Vec<_>>();
        let sql = format!(
            "UPDATE downloads SET downloaded_size = ?1, total_size = ?2, speed = ?3,
             status = ?4, completed_at = ?5, error_message = ?6
             WHERE id = ?7 AND status IN ({})",
            placeholders.join(", ")
        );
        let mut params: Vec<Box<dyn rusqlite::ToSql>> = vec![
            Box::new(downloaded_size as i64),
            Box::new(total_size as i64),
            Box::new(speed),
            Box::new(status.as_str().to_string()),
            Box::new(completed_at.map(str::to_string)),
            Box::new(error_message.map(str::to_string)),
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
             SET status = 'Queued', priority = priority + 1, aria2_gid = NULL,
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

    pub fn claim_aria2_download(&self, id: i64, gid: &str) -> Result<bool> {
        let conn = self.conn.get().map_err(|e| DatabaseError::PoolError(e.to_string()))?;
        let rows = conn.execute(
            "UPDATE downloads
             SET aria2_gid = ?1, status = 'Downloading', error_message = NULL
             WHERE id = ?2 AND status = 'Queued' AND aria2_gid IS NULL",
            params![gid, id],
        )?;
        Ok(rows == 1)
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

    pub fn clear_aria2_gid_if_current(&self, id: i64, expected: &[DownloadStatus]) -> Result<bool> {
        if expected.is_empty() {
            return Ok(false);
        }
        let conn = self.conn.get().map_err(|e| DatabaseError::PoolError(e.to_string()))?;
        let placeholders =
            (0..expected.len()).map(|index| format!("?{}", index + 2)).collect::<Vec<_>>();
        let sql = format!(
            "UPDATE downloads SET aria2_gid = NULL
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

    pub fn finish_stream_if_active(&self, id: i64, size: u64) -> Result<bool> {
        let conn = self.conn.get().map_err(|e| DatabaseError::PoolError(e.to_string()))?;
        let rows = conn.execute(
            "UPDATE downloads
             SET downloaded_size = ?1,
                 total_size = ?1,
                 speed = 0.0,
                 status = 'Completed',
                 completed_at = ?2,
                 error_message = NULL
             WHERE id = ?3 AND status IN ('Downloading', 'Merging')",
            params![size as i64, Utc::now().to_rfc3339(), id],
        )?;
        Ok(rows == 1)
    }

    pub fn pause_stream_if_active(&self, id: i64) -> Result<bool> {
        let conn = self.conn.get().map_err(|e| DatabaseError::PoolError(e.to_string()))?;
        let rows = conn.execute(
            "UPDATE downloads SET speed = 0.0, status = 'Paused'
             WHERE id = ?1 AND status IN ('Downloading', 'Merging')",
            params![id],
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

    pub fn update_download_progress(
        &self,
        id: i64,
        downloaded_size: u64,
        speed: f64,
        status: &DownloadStatus,
    ) -> Result<bool> {
        let conn = self.conn.get().map_err(|e| DatabaseError::PoolError(e.to_string()))?;
        let rows = conn.execute(
            "UPDATE downloads SET downloaded_size = ?1, speed = ?2, status = ?3
             WHERE id = ?4 AND status IN ('Downloading', 'Merging')",
            params![downloaded_size as i64, speed, status.as_str(), id],
        )?;
        Ok(rows == 1)
    }

    pub fn take_queued(&self, limit: i64) -> Result<Vec<Download>> {
        let conn = self.conn.get().map_err(|e| DatabaseError::PoolError(e.to_string()))?;
        let mut stmt = conn.prepare(
            "SELECT id, url, filename, save_path, total_size, downloaded_size, status, \
             category, speed, segments, priority, created_at, completed_at, error_message, \
             referrer, user_agent, cookies, aria2_gid, archived FROM downloads \
             WHERE status = 'Queued' AND (archived = 0 OR archived IS NULL) \
             ORDER BY priority DESC, id ASC LIMIT ?",
        )?;
        let rows = stmt.query_map(params![limit.max(0)], row_to_download)?;
        let mut downloads = Vec::new();
        for download in rows {
            downloads.push(download?);
        }
        Ok(downloads)
    }

    pub fn download_stats(&self) -> Result<(u64, u64, u64, u64, u64, u64, f64)> {
        let conn = self.conn.get().map_err(|e| DatabaseError::PoolError(e.to_string()))?;
        let mut stmt = conn.prepare(
            "SELECT status, COUNT(*), COALESCE(SUM(downloaded_size), 0), \
                    COALESCE(SUM(CASE WHEN status IN ('Downloading', 'Merging') THEN speed ELSE 0 END), 0) \
             FROM downloads \
             WHERE (archived = 0 OR archived IS NULL) \
             GROUP BY status",
        )?;
        let mut active = 0u64;
        let mut queued = 0u64;
        let mut paused = 0u64;
        let mut completed = 0u64;
        let mut failed = 0u64;
        let mut total_bytes = 0u64;
        let mut speed = 0.0f64;
        let mut rows = stmt.query([])?;
        while let Some(row) = rows.next()? {
            let status: String = row.get(0)?;
            let count: i64 = row.get(1)?;
            let bytes: i64 = row.get(2)?;
            let row_speed: f64 = row.get(3)?;
            let count = count.max(0) as u64;
            total_bytes = total_bytes.saturating_add(bytes.max(0) as u64);
            match status.as_str() {
                "Downloading" | "Merging" => {
                    active += count;
                    speed += row_speed;
                }
                "Queued" => queued += count,
                "Paused" => paused += count,
                "Completed" => completed += count,
                "Failed" => failed += count,
                _ => {}
            }
        }
        Ok((active, queued, paused, completed, failed, total_bytes, speed.max(0.0)))
    }

    pub fn get_downloads(&self, filter: &DownloadFilter) -> Result<Vec<Download>> {
        let conn = self.conn.get().map_err(|e| DatabaseError::PoolError(e.to_string()))?;
        let mut sql = String::from(
            "SELECT id, url, filename, save_path, total_size, downloaded_size, status, \
             category, speed, segments, priority, created_at, completed_at, error_message, \
             referrer, user_agent, cookies, aria2_gid, archived FROM downloads WHERE 1=1",
        );

        let mut params: Vec<Box<dyn rusqlite::ToSql>> = Vec::new();

        if let Some(ref status) = filter.status {
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

    pub fn get_download(&self, id: i64) -> Result<Download> {
        let conn = self.conn.get().map_err(|e| DatabaseError::PoolError(e.to_string()))?;
        let mut stmt = conn.prepare(
            "SELECT id, url, filename, save_path, total_size, downloaded_size, status, \
             category, speed, segments, priority, created_at, completed_at, error_message, \
             referrer, user_agent, cookies, aria2_gid, archived FROM downloads WHERE id = ?1",
        )?;

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
    let aria2_gid: Option<String> = row.get(17)?;
    let archived: bool = row.get::<_, i64>(18)? != 0;

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
        cookies,
        aria2_gid,
        archived,
    })
}
