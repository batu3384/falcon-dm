# Architecture

## Download routes

| Route | Handler | Notes |
|-------|---------|-------|
| HTTP(S) | Rust engine | Multi-connection range downloads when the server supports `Accept-Ranges`; every connection writes at its own offset in one hidden `.falcon.part` file and per-range progress persists in a `.falcon.state` sidecar, so pause, crash, retry and restart all resume byte-exactly in parallel. DNS-pinned clients; single-connection fallback (no resume) when ranges are unsupported |
| HLS (`.m3u8`) | Rust segments + ffmpeg mux | Parallel MPEG-TS segment fetch; proxy + speed limit; deterministic temp dir `hls-{id}` with segment resume. Encrypted, fMP4, byte-range and split-audio playlists are handed to yt-dlp; live playlists are refused |
| YouTube | yt-dlp | Watch URL + optional `format` field; global proxy/speed limit; real progress from output templates; browser cookies opt-in only |

## Queue

`QueueManager` polls every 500ms, limits concurrency with atomics, and cancels in-flight jobs via `tokio::sync::watch`. State persists in SQLite (WAL mode).

**Scheduler:** optional start/stop time window (overnight ranges work). Outside the window running workers are asked to stop, flush their resume state and go back to **Queued**; they continue from the same byte when the window opens. The queue tick never waits on a worker, and a stop is given up after a 15 s grace so a stuck worker cannot block the row forever. Shutdown-after-complete and dial-up modes are **not** implemented.

## Local API

Axum on `http://127.0.0.1:14201`:

| Endpoint | Purpose |
|----------|---------|
| `GET /api/health` | Liveness (unauthenticated, loopback only, `protocol: 2`) |
| `POST /api/pair` | Extension pairing by 6-digit code |
| `POST /api/ping` | Authenticated liveness for a paired extension |
| `GET /api/downloads` | Live download rows for the popup |
| `POST /api/intercept` | Captured / media download |
| `POST /api/add` | Direct URL enqueue (grabber batch, context menu) |
| `POST /api/upload` | Blob media (32 MB cap, token + Origin) |

All mutating requests require `X-Falcon-Token` and an allowed extension `Origin`.

## Duplicate protection

Enqueue uses an immediate SQLite transaction: if the same URL is already active (`Queued`, `Downloading`, `Paused`, `Merging`), the existing download ID is returned instead of creating a duplicate row.

## Settings that affect downloads

| Setting | Effect |
|---------|--------|
| **Max concurrent downloads** | Queue slot count |
| **Connections per file** | HTTP range segments and HLS segment concurrency (1–16, default 8) |
| **Speed limit (KB/s)** | HTTP (single + parallel + resume), HLS segments, yt-dlp (`--limit-rate`) |
| **Proxy** | HTTP, HLS, yt-dlp |
| **yt-dlp path** | Custom binary; empty = PATH + common locations |
| **Use browser cookies for yt-dlp** | Off by default; passes `Cookie` header to yt-dlp when enabled |
| **Download profiles** | Per-site UA, referrer, cookies (HTTPS only, encrypted at rest), save subdir |
| **Scheduler** | Active window for new work; in-flight stop grace 15 s before the row is demoted |

## Extension integration

YouTube watch/Shorts pages load a static MV3 `content_scripts` entry. Other sites inject on demand via `chrome.scripting.executeScript`. The `tabs` permission is not requested (`<all_urls>` covers tab URL).

| Control | Behavior |
|---------|----------|
| **Send browser downloads to Falcon DM** (popup/options) | Off disables automatic capture; the browser saves natively |
| **Let Chrome download when Falcon DM is closed** (options) | Off by default: a capture that cannot reach the app is cancelled. On: the browser re-downloads it instead |
| **Link grabber batch** | Up to 100 URLs per send; partial per-item results |

Manual actions (context menu, media overlay, paste URL) always show an error notification on failure; they do not use the hijack fail-closed path.

## Updates

**Settings → Check for updates** calls GitHub Releases API, compares semver to the running app, and opens the release page when newer. In-app signed auto-install (`tauri-plugin-updater`) is not shipped yet.
