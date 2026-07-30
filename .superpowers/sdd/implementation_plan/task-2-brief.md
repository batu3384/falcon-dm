# Task 2: SQLite Database Layer

## Goal
Create SQLite database layer with download models for Falcon DM. This is the persistence layer — all download metadata stored here.

## Project Context
Tauri v2 project at `/Users/batuhanyuksel/Documents/downloadmanager`. Rust backend in `src-tauri/`. Already has rusqlite dependency in Cargo.toml.

## Files to Create
- `src-tauri/src/storage/mod.rs` — module declaration
- `src-tauri/src/storage/database.rs` — Database struct + CRUD operations  
- `src-tauri/src/storage/models.rs` — Data models (Download, DownloadStatus, DownloadCategory, DownloadFilter)

## Files to Modify
- `src-tauri/src/lib.rs` — add `mod storage;` declaration

## Data Models

```rust
// models.rs
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub enum DownloadStatus {
    Queued,
    Downloading,
    Paused,
    Completed,
    Failed,
    Merging,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub enum DownloadCategory {
    Video,
    Music,
    Document,
    Archive,
    Program,
    Other,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Download {
    pub id: Option<i64>,
    pub url: String,
    pub filename: String,
    pub save_path: String,
    pub total_size: u64,
    pub downloaded_size: u64,
    pub status: DownloadStatus,
    pub category: DownloadCategory,
    pub speed: f64,
    pub segments: u32,
    pub priority: u32,
    pub created_at: String,       // ISO 8601
    pub completed_at: Option<String>,
    pub error_message: Option<String>,
    pub referrer: Option<String>,
    pub user_agent: Option<String>,
    pub cookies: Option<String>,
    pub aria2_gid: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct DownloadFilter {
    pub status: Option<DownloadStatus>,
    pub category: Option<DownloadCategory>,
    pub search: Option<String>,
}
```

## Database Operations

The `Database` struct must:
1. `init(app_data_dir: &Path) -> Result<Self>` — create/open SQLite DB, run migration
2. `insert_download(download: &Download) -> Result<i64>` — insert, return id
3. `update_download(id: i64, download: &Download) -> Result<()>` — full update
4. `update_download_progress(id: i64, downloaded_size: u64, speed: f64, status: &DownloadStatus) -> Result<()>` — lightweight progress update
5. `get_downloads(filter: &DownloadFilter) -> Result<Vec<Download>>` — filtered list
6. `get_download(id: i64) -> Result<Download>` — single download
7. `delete_download(id: i64) -> Result<()>` — delete

Use `Mutex<Connection>` for thread safety. DB file at `{app_data_dir}/falcon_dm.db`.

## SQL Schema

```sql
CREATE TABLE IF NOT EXISTS downloads (
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
CREATE INDEX IF NOT EXISTS idx_downloads_category ON downloads(category);
```

## Testing
Write at least one `#[cfg(test)]` module that tests insert + get round-trip.
Run: `cargo test --manifest-path src-tauri/Cargo.toml`

## Commit
```bash
git add -A
git commit -m "feat: sqlite database layer with download models"
```

## Interfaces
- Consumes: nothing special (rusqlite already in Cargo.toml)
- Produces: `Database` struct accessible from lib.rs, all CRUD methods listed above

## Report
Write full report to: `/Users/batuhanyuksel/Documents/downloadmanager/.superpowers/sdd/implementation_plan/task-2-report.md`
