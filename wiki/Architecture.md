# Architecture

## Download routes

| Route | Handler | Notes |
|-------|---------|-------|
| HTTP(S) | Rust engine | Multi-connection range downloads when the server proves range support with a valid `206 Content-Range`; every connection writes at its own offset in one hidden `.falcon.part` file and per-range progress persists in a `.falcon.state` sidecar. Pause, crash, retry and restart resume byte-exactly when the saved strong ETag or Last-Modified validator still matches. A server without such a validator, or a changed entity, restarts from zero rather than mixing versions. Malformed partial responses and compressed (`Content-Encoding` other than `identity`) responses are rejected. Servers without range support use one connection and cannot resume. |
| HLS (`.m3u8`) | Rust segments + native mux | Parallel MPEG-TS/fMP4 piece fetch with proxy and speed limit. AES-128 CBC segments, `EXT-X-MAP` init sections, byte ranges, discontinuities and split audio renditions are native. The per-download cache `hls-{id}.falcondm-temp` reuses only complete plaintext pieces with matching length sidecars; a rendition/key-aware playlist fingerprint detects a re-cut playlist and drops stale cache while ignoring signed-query rotation. Expired segment credentials (`401`, `403`, `410`) cause one playlist re-resolution per run. SAMPLE-AES/DRM is refused with `ERR_HLS_DRM`; live playlists are refused with `ERR_HLS_LIVE`. |
| YouTube / DASH / complex HLS | yt-dlp | Watch URL + optional `format` field; global proxy and speed limit; real progress from output templates; stable `.falcon-dm-ytdlp-{id}` work dir for pause/resume. Input URLs are validated against private and loopback targets, yt-dlp older than 2023.07.06 is refused, browser cookies are opt-in, and cancellation kills the yt-dlp process group (including ffmpeg postprocessors). Without a configured user proxy, yt-dlp is launched through Falcon's validating loopback egress proxy and child processes inherit proxy variables, so redirects, extractor requests and media fetches are rechecked before egress. |

Before any engine moves a finished file into place, the shared finalizer validates its content and can enforce an optional SHA-256 or MD5 checksum. The New Download sheet accepts bare hex or `sha256:`/`sha-256:`/`md5:` input for a single link, stores canonical lowercase `algo:hex`, verifies the finished temp file in streaming reads, and deletes that temp file on mismatch so the retry starts cleanly.

## Queue

`QueueManager` polls every 500ms, limits concurrency with atomics, and cancels in-flight jobs via `tokio::sync::watch`. State persists in SQLite (WAL mode). Transient failures are retried with backoff up to five times; progress made during a run resets the retry budget.

**Scheduler:** an optional start/stop time window plus a Monday–Sunday mask (all seven days enabled by default and for legacy settings). Overnight windows belong to the day on which they start: Tuesday 03:00 can be inside Monday 23:00–07:00 even when Tuesday is off. Outside the window running workers are asked to stop, flush their resume state and return to **Queued**; the queue tick itself never waits on a worker.

**Completion action:** after worker activity has been observed, the action fires once when no active, queued or paused downloads remain. Completed and failed rows count as done. Queued or paused rows suppress the action and consume that drain; deleting them later does not fire it until new worker activity re-arms the detector. Choices are **Do nothing**, **Put Mac to sleep**, or **Run a script**. Scripts run directly (not through a shell) and must be executable files inside the user's home folder, `/opt/homebrew/bin` or `/usr/local/bin`.

While any worker is active on macOS, Falcon DM holds a `caffeinate` idle-sleep assertion tied to its own process; the assertion is released when the queue becomes idle.

## Local API

Axum on `http://127.0.0.1:14201`:

| Endpoint | Purpose |
|----------|---------|
| `GET /api/health` | Liveness (unauthenticated, loopback only, `protocol: 3`) |
| `POST /api/pair` | Extension pairing by 6-digit code |
| `POST /api/ping` | Authenticated liveness for a paired extension |
| `GET /api/downloads` | Last 8 live/recent rows for the popup; URLs and paths are omitted |
| `GET /api/downloads/all` | Filtered, paginated rows (`status`, `category`, `search`, one-based `page`, `page_size` ≤ 200); metadata-only unless `include=url` is passed |
| `GET /api/stats` | Queue totals, current speed, free disk space and active global speed limit |
| `POST /api/intercept` | Captured / media download |
| `POST /api/add` | Direct URL enqueue (grabber batch, context menu, CLI) |
| `POST /api/upload` | Blob media (32 MiB binary cap) |
| `POST /api/pause` / `POST /api/resume` | Pause or resume one download by `{id}` |
| `POST /api/remove` | Remove one download by `{id}`; optional `delete_file` deletes a completed output file |
| `POST /api/pause_all` / `POST /api/resume_all` | Pause active work or resume paused/failed rows |
| `POST /api/speed_limit` | `{kbps}` sets the global cap; `{id,kbps}` sets a per-download cap; zero disables the global cap or clears the per-download override |

Authenticated requests require `X-Falcon-Token` plus one of two client shapes. Extension requests keep the original shape: an allowlisted `chrome-extension://` or `edge-extension://` `Origin` (or another explicitly supported extension origin). CLI/script requests use `X-Falcon-Client: cli` and must carry no `Origin` header at all. This does not replace the token: it is read from the same-user, 0600 `settings.json`. A cross-origin browser page cannot use the CLI shape because browser POSTs carry `Origin`, and `X-Falcon-Client` is a custom header that is not in the API's CORS allowlist, so the preflight fails. Any `Origin` — even an empty, invalid or undecodable one — is evaluated under the extension path instead.

Pairing requires a browser-supplied extension origin but no token. Non-health API calls share a 600 requests/minute bucket; uploads additionally have their own 10 requests/minute bucket and a 48 MiB encoded request-body cap.

## Pairing and duplicate protection

The pairing token handoff is one-shot: approval applies to one exact extension/code pair and is consumed when the token is returned. If the token is lost or rotated, the extension must be approved again; there is no silent reissue. Disconnecting an extension rotates the token.

Enqueue uses an immediate SQLite transaction. If the same canonical URL is already active (`Queued`, `Downloading`, `Paused`, `Merging`) and not archived, the existing download ID is returned instead of creating a duplicate row. Canonicalization collapses case and default-port spellings without re-encoding signed query bytes; YouTube watch URLs and format variants dedupe to the same base video.

## Refresh link

**Refresh Link…** is available on failed and paused rows. The replacement URL goes through the same public-URL validation and canonicalization as a new enqueue. For HTTP rows, the `.falcon.state` file's URL is rewritten without touching progress or the validator, so an expired signed URL can continue byte-exactly; if the new URL serves a different entity, the resume validator discards the partial data and starts over. HLS reuse is governed by the playlist fingerprint, and yt-dlp reuse by the partial files in its stable work directory, so the intended use is a fresh URL for the same content. Cookies are kept only when the refreshed target is the same HTTPS origin or the allowed YouTube→googlevideo case; other cross-host refreshes drop them. A failed row requeues after refresh; a paused row stays paused.

## Settings that affect downloads

| Setting | Effect |
|---------|--------|
| **Max concurrent downloads** | Queue slot count (1–32) |
| **Connections per file** | HTTP range segments and HLS segment concurrency (1–16, default 8; yt-dlp receives at most 8) |
| **Speed limit (KB/s)** | Decimal units (1 KB/s = 1000 B/s). One global token bucket covers HTTP and HLS and applies to running jobs; yt-dlp receives a divided `--limit-rate`. Toolbar/Settings remember the last non-zero value in webview storage, while the tray keeps a backend `speed_limit_last_kbps` mirror updated by Settings saves |
| **Proxy** | HTTP, HLS, yt-dlp. A configured proxy takes precedence for yt-dlp; when empty, yt-dlp egress instead uses Falcon's validating loopback proxy |
| **Scheduler days and times** | Weekly active window for new work |
| **Completion action/script** | One-shot action when the queue drains |
| **yt-dlp path** | Custom binary; empty = PATH + common install locations |
| **Let yt-dlp use your Chrome sign-in** | Off by default; uses `--cookies-from-browser chrome` only when no exact-host download cookies apply |
| **Download profiles** | Per-site UA, referrer, cookies (HTTPS only, encrypted at rest), save subdir |

## Browser capture rules

Automatic capture is limited to the extension list configured in the extension options. The default list covers common media, archive, document, image and installer types; separators can be commas, spaces or newlines, and an empty list captures nothing. The per-site **Never capture on these sites** list accepts one host per line; `*.example.com` or `.example.com` covers the base host and subdomains. These rules apply only to automatic browser capture — explicit context-menu sends still work.

## Extension integration

YouTube watch/Shorts pages load a static MV3 `content_scripts` entry. Other sites inject on demand via `chrome.scripting.executeScript`. The `tabs` permission is not requested (`<all_urls>` covers tab URL).

| Control | Behavior |
|---------|----------|
| **Send browser downloads to Falcon DM** (popup/options) | Off disables automatic capture; the browser saves natively |
| **Let Chrome download when Falcon DM is closed** (options) | Off by default: a capture that cannot reach the app is cancelled. On: the browser starts a replacement download instead |
| **Link grabber batch** | Up to 100 URLs per send; partial per-item results |

Manual actions (context menu, media overlay, paste URL) always show an error notification on failure; they do not use the automatic-capture fail-closed path.

## Updates

**Settings → Check for Updates** calls the GitHub Releases API, compares semver to the running app, and opens the release page when newer. In-app signed auto-install (`tauri-plugin-updater`) is not shipped yet.
