use crate::storage::models::{Download, DownloadCategory, DownloadFilter, DownloadStatus};
use chrono::Utc;
use r2d2::Pool;
use r2d2_sqlite::SqliteConnectionManager;
use rusqlite::{params, Row};
use rusqlite_migration::{Migrations, M};
use std::fmt;
use std::fs;
use std::path::Path;
use std::sync::Arc;

#[derive(Debug)]
pub enum DatabaseError {
    Sqlite(rusqlite::Error),
    Io(std::io::Error),
    LockError,
    PoolError(String),
    NotFound(i64),
}

impl fmt::Display for DatabaseError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            DatabaseError::Sqlite(err) => write!(f, "SQLite error: {}", err),
            DatabaseError::Io(err) => write!(f, "IO error: {}", err),
            DatabaseError::LockError => write!(f, "Failed to acquire database lock"),
            DatabaseError::PoolError(err) => write!(f, "DB pool error: {}", err),
            DatabaseError::NotFound(id) => write!(f, "Download with id {} not found", id),
        }
    }
}

impl std::error::Error for DatabaseError {}

impl From<rusqlite::Error> for DatabaseError {
    fn from(err: rusqlite::Error) -> Self {
        DatabaseError::Sqlite(err)
    }
}

impl From<std::io::Error> for DatabaseError {
    fn from(err: std::io::Error) -> Self {
        DatabaseError::Io(err)
    }
}

pub type Result<T> = std::result::Result<T, DatabaseError>;

#[derive(Clone)]
pub struct Database {
    // ponytail: connection pool (default 8 conns) replaces the single shared
    // Connection+Mutex. The progress-poll loop, queue tick and HTTP enqueue all
    // contend(ed) on that one lock; with a pool they run in parallel (WAL allows
    // multiple readers + one writer). Each `self.conn.get()` borrows a connection
    // for the duration of a single query.
    conn: Arc<Pool<SqliteConnectionManager>>,
}

impl Database {
    pub fn init(app_data_dir: &Path) -> Result<Self> {
        if app_data_dir != Path::new(":memory:") {
            fs::create_dir_all(app_data_dir)?;
        }

        let db_path = if app_data_dir == Path::new(":memory:") {
            app_data_dir.to_path_buf()
        } else {
            app_data_dir.join("falcon_dm.db")
        };

        let manager = SqliteConnectionManager::file(&db_path).with_init(|c| {
            c.execute_batch(
                // PRAGMAs applied to every pooled connection on creation.
                "PRAGMA journal_mode=WAL;
                 PRAGMA busy_timeout=5000;
                 PRAGMA foreign_keys = ON;",
            )
        });
        let pool = Pool::builder()
            .max_size(8)
            .build(manager)
            .map_err(|e| DatabaseError::PoolError(e.to_string()))?;

        let db = Self { conn: Arc::new(pool) };
        db.run_migrations()?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(&db_path, fs::Permissions::from_mode(0o600))?;
        }
        Ok(db)
    }

    pub fn in_memory() -> Result<Self> {
        let manager = SqliteConnectionManager::memory().with_init(|c| {
            c.execute_batch(
                "PRAGMA journal_mode=WAL;
                 PRAGMA busy_timeout=5000;
                 PRAGMA foreign_keys = ON;",
            )
        });
        let pool = Pool::builder()
            .max_size(4)
            .build(manager)
            .map_err(|e| DatabaseError::PoolError(e.to_string()))?;

        let db = Self { conn: Arc::new(pool) };
        db.run_migrations()?;
        Ok(db)
    }

    fn run_migrations(&self) -> Result<()> {
        let mut conn = self.conn.get().map_err(|e| DatabaseError::PoolError(e.to_string()))?;
        // ponytail: versioned migrations. Previously the schema was created with
        // bare `CREATE TABLE IF NOT EXISTS` and no `PRAGMA user_version`, so any
        // future ALTER/ADD-COLUMN could not be applied to existing user databases.
        // `rusqlite_migration` tracks the current version via user_version and runs
        // only the pending migrations. Existing v0 databases (no user_version) are
        // handled by the v1 baseline: it re-runs the idempotent CREATE statements.
        let migrations = Migrations::new(vec![
            M::up(
                "CREATE TABLE IF NOT EXISTS downloads (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                url TEXT NOT NULL,
                filename TEXT NOT NULL,
                save_path TEXT NOT NULL,
                total_size INTEGER DEFAULT 0,
                downloaded_size INTEGER DEFAULT 0,
                status TEXT DEFAULT 'Queued',
                category TEXT DEFAULT 'Other',
                speed REAL DEFAULT 0.0,
                segments INTEGER DEFAULT 8,
                priority INTEGER DEFAULT 1,
                created_at TEXT NOT NULL,
                completed_at TEXT,
                error_message TEXT,
                referrer TEXT,
                user_agent TEXT,
                cookies TEXT,
                aria2_gid TEXT
            );
            CREATE INDEX IF NOT EXISTS idx_downloads_status ON downloads(status);
            CREATE INDEX IF NOT EXISTS idx_downloads_category ON downloads(category);",
            ),
            // ponytail: v2 — add archived flag so completed downloads can be
            // archived (hidden from the active list) without being deleted.
            // Existing rows default to archived=0 (visible).
            M::up(
                "ALTER TABLE downloads ADD COLUMN archived INTEGER NOT NULL DEFAULT 0;
                 CREATE INDEX IF NOT EXISTS idx_downloads_archived ON downloads(archived);",
            ),
        ]);
        migrations.to_latest(&mut conn).map_err(|e| {
            DatabaseError::Sqlite(rusqlite::Error::ToSqlConversionFailure(Box::new(e)))
        })?;
        Ok(())
    }

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
                sql.push_str(" AND (filename LIKE ? OR url LIKE ?)");
                let pattern = format!("%{}%", search.trim());
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

#[cfg(test)]
mod tests {
    use super::*;

    fn create_test_download(name: &str) -> Download {
        Download {
            id: None,
            url: format!("https://example.com/{}", name),
            filename: name.to_string(),
            save_path: format!("/downloads/{}", name),
            total_size: 1024 * 1024,
            downloaded_size: 0,
            status: DownloadStatus::Queued,
            category: DownloadCategory::from_filename(name),
            speed: 0.0,
            segments: 8,
            priority: 1,
            created_at: "2026-07-30T13:00:00Z".to_string(),
            completed_at: None,
            error_message: None,
            referrer: Some("https://example.com".to_string()),
            user_agent: Some("FalconDM/1.0".to_string()),
            cookies: None,
            aria2_gid: None,
            archived: false,
        }
    }

    #[test]
    fn test_database_init_and_crud() {
        let db = Database::in_memory().expect("Failed to create in-memory database");

        // Insert
        let download = create_test_download("sample.mp4");
        let id = db.insert_download(&download).expect("Insert failed");
        assert!(id > 0);

        // Get single
        let fetched = db.get_download(id).expect("Get single failed");
        assert_eq!(fetched.id, Some(id));
        assert_eq!(fetched.filename, "sample.mp4");
        assert_eq!(fetched.category, DownloadCategory::Video);
        assert_eq!(fetched.status, DownloadStatus::Queued);

        // Update progress
        db.set_status_if_current(id, &[DownloadStatus::Queued], &DownloadStatus::Downloading)
            .unwrap();
        db.update_download_progress(id, 512 * 1024, 1024.5, &DownloadStatus::Downloading)
            .expect("Update progress failed");
        let updated_progress = db.get_download(id).expect("Get after progress update failed");
        assert_eq!(updated_progress.downloaded_size, 512 * 1024);
        assert_eq!(updated_progress.speed, 1024.5);
        assert_eq!(updated_progress.status, DownloadStatus::Downloading);

        // Full Update
        let mut to_update = updated_progress.clone();
        to_update.status = DownloadStatus::Completed;
        to_update.completed_at = Some("2026-07-30T13:05:00Z".to_string());
        db.update_download(id, &to_update).expect("Full update failed");

        let updated_full = db.get_download(id).expect("Get after full update failed");
        assert_eq!(updated_full.status, DownloadStatus::Completed);
        assert_eq!(updated_full.completed_at, Some("2026-07-30T13:05:00Z".to_string()));

        // Delete
        db.delete_download(id).expect("Delete failed");
        assert!(db.get_download(id).is_err());
    }

    #[test]
    fn test_filtering() {
        let db = Database::in_memory().expect("Failed to create in-memory db");

        let d1 = create_test_download("video1.mp4");
        let d2 = create_test_download("song1.mp3");
        let d3 = create_test_download("doc1.pdf");

        let id1 = db.insert_download(&d1).unwrap();
        let id2 = db.insert_download(&d2).unwrap();
        let _id3 = db.insert_download(&d3).unwrap();

        db.set_status_if_current(id1, &[DownloadStatus::Queued], &DownloadStatus::Downloading)
            .unwrap();
        db.update_download_progress(id1, 100, 50.0, &DownloadStatus::Downloading).unwrap();
        db.set_status_if_current(id2, &[DownloadStatus::Queued], &DownloadStatus::Downloading)
            .unwrap();
        db.update_download_progress(id2, 100, 0.0, &DownloadStatus::Paused).unwrap();

        // Filter by category
        let video_filter =
            DownloadFilter { category: Some(DownloadCategory::Video), ..Default::default() };
        let videos = db.get_downloads(&video_filter).unwrap();
        assert_eq!(videos.len(), 1);
        assert_eq!(videos[0].filename, "video1.mp4");

        // Filter by status
        let paused_filter =
            DownloadFilter { status: Some(DownloadStatus::Paused), ..Default::default() };
        let paused = db.get_downloads(&paused_filter).unwrap();
        assert_eq!(paused.len(), 1);
        assert_eq!(paused[0].filename, "song1.mp3");

        // Filter by search term
        let search_filter =
            DownloadFilter { search: Some("doc1".to_string()), ..Default::default() };
        let search_results = db.get_downloads(&search_filter).unwrap();
        assert_eq!(search_results.len(), 1);
        assert_eq!(search_results[0].filename, "doc1.pdf");
    }

    #[test]
    fn test_init_with_file_path() {
        let temp_dir = std::env::temp_dir().join(format!("falcon_test_{}", uuid::Uuid::new_v4()));
        let db = Database::init(&temp_dir).expect("Failed to init db with file path");

        let download = create_test_download("archive.zip");
        let id = db.insert_download(&download).expect("Insert failed");
        assert!(id > 0);

        let fetched = db.get_download(id).unwrap();
        assert_eq!(fetched.filename, "archive.zip");
        assert_eq!(fetched.category, DownloadCategory::Archive);

        let _ = fs::remove_dir_all(&temp_dir);
    }

    #[cfg(unix)]
    #[test]
    fn database_file_is_private() {
        let temp_dir =
            std::env::temp_dir().join(format!("falcon_db_mode_{}", uuid::Uuid::new_v4()));
        let _db = Database::init(&temp_dir).expect("Failed to init db");
        use std::os::unix::fs::PermissionsExt;
        let mode = fs::metadata(temp_dir.join("falcon_dm.db")).unwrap().permissions().mode();
        assert_eq!(mode & 0o777, 0o600);
        let _ = fs::remove_dir_all(&temp_dir);
    }

    #[test]
    fn test_wal_mode_enabled() {
        let temp_dir = std::env::temp_dir().join(format!("falcon_wal_{}", uuid::Uuid::new_v4()));
        let db = Database::init(&temp_dir).expect("Failed to init db");
        assert_eq!(db.journal_mode().expect("journal_mode query"), "wal");
        let _ = fs::remove_dir_all(&temp_dir);
    }

    #[test]
    fn test_pagination() {
        let db = Database::in_memory().expect("Failed to create in-memory db");
        for i in 0..10u32 {
            let d = create_test_download(&format!("page{i}.zip"));
            db.insert_download(&d).expect("insert");
        }
        let first = db
            .get_downloads(&DownloadFilter {
                limit: Some(3),
                offset: Some(0),
                ..Default::default()
            })
            .unwrap();
        assert_eq!(first.len(), 3);
        let second = db
            .get_downloads(&DownloadFilter {
                limit: Some(3),
                offset: Some(3),
                ..Default::default()
            })
            .unwrap();
        assert_eq!(second.len(), 3);
        // ordered by id DESC → pages must not share the head id
        assert_ne!(first[0].id, second[0].id);
        let all = db.get_downloads(&DownloadFilter::default()).unwrap();
        assert_eq!(all.len(), 10);
    }

    #[test]
    fn cursor_pagination_ignores_rows_inserted_after_first_page() {
        let db = Database::in_memory().expect("Failed to create in-memory db");
        for i in 0..6u32 {
            db.insert_download(&create_test_download(&format!("cursor{i}.zip"))).unwrap();
        }
        let first =
            db.get_downloads(&DownloadFilter { limit: Some(3), ..Default::default() }).unwrap();
        let cursor = first.last().and_then(|download| download.id).unwrap();
        db.insert_download(&create_test_download("newest.zip")).unwrap();

        let second = db
            .get_downloads(&DownloadFilter {
                limit: Some(3),
                before_id: Some(cursor),
                ..Default::default()
            })
            .unwrap();
        assert_eq!(second.len(), 3);
        assert!(second.iter().all(|download| download.id.unwrap() < cursor));
    }

    #[test]
    fn priority_update_does_not_overwrite_worker_fields() {
        let db = Database::in_memory().unwrap();
        let id = db.insert_download(&create_test_download("priority.zip")).unwrap();
        db.set_status_if_current(id, &[DownloadStatus::Queued], &DownloadStatus::Downloading)
            .unwrap();
        db.update_download_progress(id, 512, 42.0, &DownloadStatus::Downloading).unwrap();

        assert!(db.adjust_priority(id, true).unwrap());
        let updated = db.get_download(id).unwrap();
        assert_eq!(updated.priority, 2);
        assert_eq!(updated.status, DownloadStatus::Downloading);
        assert_eq!(updated.downloaded_size, 512);
        assert_eq!(updated.speed, 42.0);
    }

    // ponytail: verify the versioned migration actually advanced user_version.
    // Without this, a regression to the old CREATE-IF-NOT-EXISTS approach would
    // silently make future migrations no-ops on existing databases.
    #[test]
    fn test_migration_sets_user_version() {
        let db = Database::in_memory().expect("Failed to create in-memory db");
        let conn = db.conn.get().expect("pool get");
        let version: u32 = conn
            .query_row("PRAGMA user_version", [], |row| row.get(0))
            .expect("user_version query");
        assert!(version >= 1, "user_version should be >= 1 after migration, got {version}");
    }

    #[test]
    fn test_migration_is_idempotent() {
        let temp_dir = std::env::temp_dir().join(format!("falcon_mig_{}", uuid::Uuid::new_v4()));
        // First init creates the schema + sets user_version.
        let db1 = Database::init(&temp_dir).expect("first init");
        drop(db1);
        // Second init on the same DB must not error and must keep the data.
        let db2 = Database::init(&temp_dir).expect("second init");
        let d = create_test_download("repeat.mp4");
        let id = db2.insert_download(&d).expect("insert after re-init");
        assert!(id > 0);
        let _ = fs::remove_dir_all(&temp_dir);
    }

    #[test]
    fn status_transition_rejects_wrong_current_state() {
        let db = Database::in_memory().expect("Failed to create in-memory db");
        let mut download = create_test_download("active.mp4");
        download.status = DownloadStatus::Downloading;
        let id = db.insert_download(&download).unwrap();
        assert!(!db
            .set_status_if_current(id, &[DownloadStatus::Completed], &DownloadStatus::Paused)
            .unwrap());
    }

    #[test]
    fn explicit_non_archived_filter_excludes_archived_rows() {
        let db = Database::in_memory().unwrap();
        let visible_id = db.insert_download(&create_test_download("visible.mp4")).unwrap();
        let mut archived = create_test_download("archived.mp4");
        archived.archived = true;
        let archived_id = db.insert_download(&archived).unwrap();

        let visible = db
            .get_downloads(&DownloadFilter { archived: Some(false), ..Default::default() })
            .unwrap();
        let archived_rows = db
            .get_downloads(&DownloadFilter { archived: Some(true), ..Default::default() })
            .unwrap();
        assert_eq!(visible.iter().map(|row| row.id).collect::<Vec<_>>(), vec![Some(visible_id)]);
        assert_eq!(
            archived_rows.iter().map(|row| row.id).collect::<Vec<_>>(),
            vec![Some(archived_id)]
        );
    }

    #[test]
    fn status_transition_does_not_overwrite_user_fields() {
        let db = Database::in_memory().unwrap();
        let mut download = create_test_download("state.mp4");
        download.status = DownloadStatus::Downloading;
        download.priority = 9;
        download.cookies = Some("sid=secret".into());
        download.archived = true;
        let id = db.insert_download(&download).unwrap();

        assert!(db
            .set_status_if_current(id, &[DownloadStatus::Downloading], &DownloadStatus::Paused)
            .unwrap());
        let updated = db.get_download(id).unwrap();
        assert_eq!(updated.status, DownloadStatus::Paused);
        assert_eq!(updated.priority, 9);
        assert_eq!(updated.cookies.as_deref(), Some("sid=secret"));
        assert!(updated.archived);
    }

    #[test]
    fn legacy_aria2_gid_can_be_cleared_atomically() {
        let db = Database::in_memory().unwrap();
        let mut download = create_test_download("legacy.mp4");
        download.aria2_gid = Some("gid-1".into());
        let id = db.insert_download(&download).unwrap();

        assert!(db.clear_aria2_gid_if_current(id, &[DownloadStatus::Queued]).unwrap());
        assert!(db.get_download(id).unwrap().aria2_gid.is_none());
    }

    #[test]
    fn clear_session_cookies_removes_storage_only_cookie() {
        let db = Database::in_memory().unwrap();
        let mut download = create_test_download("cookie.mp4");
        download.cookies = Some("sid=secret".into());
        download.status = DownloadStatus::Failed;
        let id = db.insert_download(&download).unwrap();

        assert!(db.clear_session_cookies(id).unwrap());
        assert!(db.get_download(id).unwrap().cookies.is_none());
    }

    #[test]
    fn aria2_claim_is_single_winner() {
        let db = Database::in_memory().unwrap();
        let download = create_test_download("claim.bin");
        let id = db.insert_download(&download).unwrap();
        assert!(db.claim_aria2_download(id, "gid-1").unwrap());
        assert!(!db.claim_aria2_download(id, "gid-2").unwrap());
        assert_eq!(db.get_download(id).unwrap().aria2_gid.as_deref(), Some("gid-1"));
    }

    #[test]
    fn completed_transition_does_not_override_paused_state() {
        let db = Database::in_memory().unwrap();
        let mut download = create_test_download("stream.mp4");
        download.status = DownloadStatus::Paused;
        let id = db.insert_download(&download).unwrap();
        assert!(!db.finish_stream_if_active(id, 100).unwrap());
        assert_eq!(db.get_download(id).unwrap().status, DownloadStatus::Paused);
    }
}
