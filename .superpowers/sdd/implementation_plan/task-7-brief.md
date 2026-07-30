# Task 7: Chrome Browser Extension

## Goal
Create a Chrome extension (Manifest V3) that intercepts browser downloads and sends them to the Falcon DM application. Implement a local HTTP server in the Rust backend to receive these requests.

## Project Context
- Tauri v2 + React project at `/Users/batuhanyuksel/Documents/downloadmanager`.
- The Rust backend manages downloads via aria2 and SQLite.

## Requirements

### 1. Rust Backend (Local HTTP API)
- Add a tiny HTTP server to the Tauri Rust backend running on `127.0.0.1:14201`. You can use `axum` or `warp` or just raw `hyper`. Add the dependency to `Cargo.toml`.
- Expose a `POST /api/add` endpoint that accepts JSON:
  ```json
  {
    "url": "https://example.com/file.zip",
    "filename": "file.zip",
    "referrer": "https://example.com",
    "user_agent": "Mozilla/5.0...",
    "cookies": "session=123"
  }
  ```
- When this endpoint is hit, it should use the `QueueManager` or `Database` to add the download and start it via aria2.
- Spawn this HTTP server in a tokio background task during `tauri::Builder::setup`.

### 2. Chrome Extension (Manifest V3)
- Create the extension in a new folder: `/Users/batuhanyuksel/Documents/downloadmanager/extensions/chrome/`.
- `manifest.json`:
  - `manifest_version`: 3
  - `name`: "Falcon DM Integration"
  - `permissions`: ["downloads", "downloads.ui", "storage", "contextMenus", "cookies"]
  - `host_permissions`: ["<all_urls>", "http://127.0.0.1:14201/*"]
  - `background`: { "service_worker": "background.js" }
- `background.js`:
  - Listen to `chrome.downloads.onDeterminingFilename`.
  - Cancel the browser download (`suggest({ cancel: true })`).
  - Extract the URL, filename, referrer, user_agent (from `navigator.userAgent`), and get cookies using `chrome.cookies.getAll`.
  - `fetch('http://127.0.0.1:14201/api/add', { method: 'POST', body: ... })`.
  - *Fallback*: If the fetch fails (app is closed), log an error. (We will add custom URL scheme wake-up in a later macOS integration task, just focus on the HTTP server for now).
- Add a Context Menu item: "Download with Falcon DM" for links and media.

## Testing
- Run `rtk cargo check --manifest-path src-tauri/Cargo.toml`.
- (Optional) You can't easily test the Chrome extension inside the sandbox, but verify the code is syntactically correct and the Rust HTTP server compiles.

## Commit
```bash
rtk git add -A
rtk git commit -m "feat: chrome extension and local http api integration"
```

## Report
Write to: `/Users/batuhanyuksel/Documents/downloadmanager/.superpowers/sdd/implementation_plan/task-7-report.md`
