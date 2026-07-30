# Task 3: aria2 Sidecar Integration

## Goal
Integrate aria2c as a sidecar subprocess for the download engine. aria2's JSON-RPC interface will be used to manage downloads. Create Tauri commands that the frontend can call.

## Project Context
- Tauri v2 project at `/Users/batuhanyuksel/Documents/downloadmanager`
- Task 2 completed: SQLite database layer exists at `src-tauri/src/storage/` with `Database` struct, `Download` model, CRUD methods
- `lib.rs` has `mod storage;` and a `greet` command

## Files to Create
- `src-tauri/src/download/mod.rs` — module declaration
- `src-tauri/src/download/engine.rs` — Aria2Engine struct with RPC client

## Files to Modify
- `src-tauri/src/lib.rs` — add `mod download;`, create AppState, register Tauri commands, start aria2 on app launch

## Architecture

The Aria2Engine:
1. Spawns `aria2c` subprocess on start with RPC enabled
2. Communicates via HTTP JSON-RPC at `http://127.0.0.1:6800/jsonrpc`
3. Uses a random RPC secret token for security
4. Frontend calls Tauri commands → Rust calls aria2 RPC → aria2 handles the download

## Aria2Engine Implementation

```rust
use reqwest::Client;
use serde_json::{json, Value};
use std::process::Child;
use std::sync::Mutex;

pub struct Aria2Options {
    pub dir: String,
    pub filename: String,
    pub split: u32,           // number of segments (default 16)
    pub max_connections: u32, // max connections per server (default 16)
    pub headers: Vec<String>, // additional HTTP headers
    pub referrer: Option<String>,
    pub user_agent: Option<String>,
}

pub struct Aria2Engine {
    client: Client,
    rpc_url: String,
    secret: String,
    process: Mutex<Option<Child>>,
}

impl Aria2Engine {
    // start(aria2_path: &str) -> Result<Self> — spawn aria2c daemon
    // stop(&self) -> Result<()> — kill aria2c process
    // add_download(&self, url: &str, options: Aria2Options) -> Result<String> — returns GID
    // pause(&self, gid: &str) -> Result<()>
    // resume(&self, gid: &str) -> Result<()>
    // remove(&self, gid: &str) -> Result<()>
    // get_status(&self, gid: &str) -> Result<Value> — returns aria2 tellStatus response
    // get_global_stat(&self) -> Result<Value> — returns global speed stats
}
```

## aria2 Startup Args
```
aria2c --enable-rpc=true --rpc-listen-port=6800 --rpc-secret=<random_uuid>
       --max-concurrent-downloads=10 --max-connection-per-server=16
       --split=16 --min-split-size=1M --continue=true
       --auto-file-renaming=false --allow-overwrite=true
       --quiet=true --daemon=false
```

## aria2 RPC Methods to Implement
- `aria2.addUri` — add download URL with options
- `aria2.pause` — pause download by GID
- `aria2.unpause` — resume download by GID
- `aria2.remove` — remove download by GID
- `aria2.tellStatus` — get download status by GID (returns completedLength, totalLength, downloadSpeed, connections, etc.)
- `aria2.getGlobalStat` — get global download/upload speed

RPC call format:
```json
{
  "jsonrpc": "2.0",
  "id": "falcon-dm",
  "method": "aria2.addUri",
  "params": ["token:<secret>", ["<url>"], {"dir": "/path", "out": "file.zip", "split": "16"}]
}
```

## AppState and Tauri Commands

Create an AppState struct that holds both Database and Aria2Engine:

```rust
pub struct AppState {
    pub db: storage::database::Database,
    pub engine: download::engine::Aria2Engine,
}
```

Register Tauri commands:
- `add_download(url: String, filename: String, save_path: String)` — adds to aria2 + saves to DB, returns Download
- `pause_download(id: i64)` — pauses in aria2 + updates DB status
- `resume_download(id: i64)` — resumes in aria2 + updates DB status
- `remove_download(id: i64)` — removes from aria2 + deletes from DB
- `get_downloads(filter: DownloadFilter)` — returns downloads from DB
- `get_download_status(id: i64)` — returns current status from aria2 (live speed/progress)

IMPORTANT: For now, since we may not have aria2c binary installed, make the engine startup graceful — if aria2c is not found, log a warning but don't crash. The app should work without aria2 (just DB operations). We'll bundle the binary later.

## Testing
- `cargo check` should pass
- Unit test for RPC call construction
- If aria2 is available (check with `which aria2c`), test actual RPC communication

## Commit
```bash
rtk git add -A
rtk git commit -m "feat: aria2 sidecar integration with RPC client"
```

## Interfaces
- Consumes: `Database` from Task 2 (storage::database::Database)
- Produces: `Aria2Engine` struct, `AppState`, Tauri commands (add_download, pause_download, resume_download, remove_download, get_downloads, get_download_status)

## Report
Write to: `/Users/batuhanyuksel/Documents/downloadmanager/.superpowers/sdd/implementation_plan/task-3-report.md`
