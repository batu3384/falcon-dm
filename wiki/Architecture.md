# Architecture

## Download routes

| Route | Handler | Notes |
|-------|---------|-------|
| HTTP(S) | Rust engine | Multi-connection range downloads when server supports `Accept-Ranges`; resumes with parallel ranges when `.falcon.part` exists and remaining bytes ≥ ~512 KB; DNS-pinned clients; single-connection fallback |
| HLS (`.m3u8`) | Rust segments + ffmpeg mux | Parallel segment fetch; proxy + speed limit; deterministic temp dir `hls-{id}` with segment resume |
| YouTube | yt-dlp | Watch URL + optional `format` field; global proxy/speed limit; browser cookies opt-in only |
| Legacy / special | aria2c sidecar | Session recovery and compatibility |

## Queue

`QueueManager` polls every 500ms, limits concurrency with atomics, and cancels in-flight jobs via `tokio::sync::watch`. State persists in SQLite (WAL mode).

**Scheduler:** optional start/stop time window gates new work. Shutdown-after-complete and dial-up modes are **not** implemented.

## Local API

Axum on `http://127.0.0.1:14201`:

| Endpoint | Purpose |
|----------|---------|
| `GET /api/health` | Liveness |
| `POST /api/pair` | Extension pairing |
| `POST /api/intercept` | Hijacked / media download |
| `POST /api/add` | Direct URL enqueue (grabber batch, context menu) |

All mutating requests require `X-Falcon-Token` and an allowed extension `Origin`.

## Duplicate protection

Enqueue uses an immediate SQLite transaction: if the same URL is already active (`Queued`, `Downloading`, `Paused`, `Merging`), the existing download ID is returned instead of creating a duplicate row.

## Settings that affect downloads

| Setting | Effect |
|---------|--------|
| **Max concurrent downloads** | Queue slot count |
| **Max connections per server** | HTTP range segments and HLS segment concurrency (1–16) |
| **Speed limit (KB/s)** | HTTP (single + parallel + resume), HLS segments, yt-dlp (`--limit-rate`) |
| **Proxy** | HTTP, HLS, yt-dlp |
| **yt-dlp path** | Custom binary; empty = PATH + common locations |
| **Use browser cookies for yt-dlp** | Off by default; passes `Cookie` header to yt-dlp when enabled |
| **Download profiles** | Per-site UA, referrer, cookies (HTTPS only), save subdir |
| **Scheduler** | Active window for queue processing |

## Extension integration

| Control | Behavior |
|---------|----------|
| **Pause** (popup) | Disables automatic download hijack; browser saves natively |
| **Block when offline** (popup/options) | On hijack failure, `cancel: true` instead of fail-open browser download |
| **Link grabber batch** | Up to 100 URLs per send; partial per-item results |

Manual actions (context menu, media overlay, paste URL) always show an error notification on failure; they do not use the hijack fail-closed path.

## Updates

**Settings → Check for updates** calls GitHub Releases API, compares semver to the running app, and opens the release page when newer. In-app signed auto-install (`tauri-plugin-updater`) is not shipped yet.
