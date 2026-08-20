# Architecture

## Download routes

| Route | Handler | Notes |
|-------|---------|-------|
| HTTP(S) | Rust engine | Multi-connection range downloads when server supports `Accept-Ranges`; DNS-pinned clients; single-connection fallback |
| HLS (`.m3u8`) | Rust segments + ffmpeg mux | Parallel segment fetch |
| YouTube | yt-dlp | Watch URL + optional `format` field; not bundled |
| Legacy / special | aria2c sidecar | Session recovery and compatibility |

## Queue

`QueueManager` polls every 500ms, limits concurrency with atomics, and cancels in-flight jobs via `tokio::sync::watch`. State persists in SQLite (WAL mode).

## Local API

Axum on `http://127.0.0.1:14201`:

| Endpoint | Purpose |
|----------|---------|
| `GET /api/health` | Liveness |
| `POST /api/pair` | Extension pairing |
| `POST /api/intercept` | Hijacked / media download |
| `POST /api/add` | Direct URL enqueue |

All mutating requests require `X-Falcon-Token` and an allowed extension `Origin`.

## Duplicate protection

Enqueue uses an immediate SQLite transaction: if the same URL is already active (`Queued`, `Downloading`, `Paused`, `Merging`), the existing download ID is returned instead of creating a duplicate row.

## Settings that affect downloads

- **Max concurrent downloads** — queue slot count
- **Max connections per server** — HTTP range segments and HLS segment concurrency (1–16)
- **Speed limit (KB/s)** — applies to HTTP single and parallel paths
- **Proxy** — passed to HTTP/HLS clients
