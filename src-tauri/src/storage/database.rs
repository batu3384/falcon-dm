#[cfg(test)]
use crate::storage::models::{Download, DownloadCategory, DownloadFilter, DownloadStatus};
use r2d2::Pool;
use r2d2_sqlite::SqliteConnectionManager;
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
    pub(crate) conn: Arc<Pool<SqliteConnectionManager>>,
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
            M::up(
                "DELETE FROM downloads
                 WHERE archived = 0
                   AND status IN ('Queued', 'Downloading', 'Paused', 'Merging')
                   AND id NOT IN (
                     SELECT MAX(id)
                     FROM downloads
                     WHERE archived = 0
                       AND status IN ('Queued', 'Downloading', 'Paused', 'Merging')
                     GROUP BY url
                   );
                 CREATE UNIQUE INDEX IF NOT EXISTS idx_downloads_active_url_unique
                   ON downloads(url)
                   WHERE status IN ('Queued', 'Downloading', 'Paused', 'Merging')
                     AND archived = 0;",
            ),
        ]);
        migrations.to_latest(&mut conn).map_err(|e| {
            DatabaseError::Sqlite(rusqlite::Error::ToSqlConversionFailure(Box::new(e)))
        })?;
        Ok(())
    }
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

    #[test]
    fn search_treats_percent_as_literal() {
        let db = Database::in_memory().unwrap();
        let mut hit = create_test_download("report%.pdf");
        hit.filename = "report%.pdf".into();
        db.insert_download(&hit).unwrap();
        let mut miss = create_test_download("reportX.pdf");
        miss.filename = "reportX.pdf".into();
        db.insert_download(&miss).unwrap();
        let rows = db
            .get_downloads(&DownloadFilter { search: Some("report%".into()), ..Default::default() })
            .unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].filename, "report%.pdf");
    }

    #[test]
    fn take_queued_orders_by_priority_with_limit() {
        let db = Database::in_memory().unwrap();
        let mut low = create_test_download("low.bin");
        low.priority = 1;
        db.insert_download(&low).unwrap();
        let mut high = create_test_download("high.bin");
        high.priority = 9;
        db.insert_download(&high).unwrap();
        let rows = db.take_queued(1).unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].filename, "high.bin");
    }

    #[test]
    fn download_stats_counts_paused() {
        let db = Database::in_memory().unwrap();
        let mut paused = create_test_download("paused.bin");
        paused.status = DownloadStatus::Paused;
        paused.downloaded_size = 10;
        db.insert_download(&paused).unwrap();
        let (active, queued, paused_count, completed, failed, bytes, _speed) =
            db.download_stats().unwrap();
        assert_eq!((active, queued, paused_count, completed, failed), (0, 0, 1, 0, 0));
        assert_eq!(bytes, 10);
    }

    #[test]
    fn find_active_download_id_by_url_returns_latest_active_only() {
        let db = Database::in_memory().unwrap();
        let url = "https://cdn.example.com/shared.bin";

        let mut completed = create_test_download("done.bin");
        completed.url = url.to_string();
        completed.status = DownloadStatus::Completed;
        db.insert_download(&completed).unwrap();

        let mut active = create_test_download("active.bin");
        active.url = url.to_string();
        active.status = DownloadStatus::Downloading;
        let active_id = db.insert_download(&active).unwrap();

        assert_eq!(db.find_active_download_id_by_url(url).unwrap(), Some(active_id));

        let only_completed_url = "https://cdn.example.com/only-completed.bin";
        let mut only_completed = create_test_download("only.bin");
        only_completed.url = only_completed_url.to_string();
        only_completed.status = DownloadStatus::Completed;
        db.insert_download(&only_completed).unwrap();
        assert_eq!(db.find_active_download_id_by_url(only_completed_url).unwrap(), None);
    }

    #[test]
    fn insert_download_deduped_returns_existing_active_url() {
        let db = Database::in_memory().unwrap();
        let url = "https://cdn.example.com/dedup.bin";

        let mut first = create_test_download("first.bin");
        first.url = url.to_string();
        first.status = DownloadStatus::Downloading;
        let first_id = db.insert_download(&first).unwrap();

        let mut second = create_test_download("second.bin");
        second.url = url.to_string();
        second.status = DownloadStatus::Queued;
        match db.insert_download_deduped(&second).unwrap() {
            crate::storage::InsertDownloadResult::Existing(id) => assert_eq!(id, first_id),
            crate::storage::InsertDownloadResult::Created(_) => {
                panic!("expected existing active download")
            }
        }
    }
}
