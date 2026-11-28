<p align="center">
  <img src="src/assets/icon.png" width="128" alt="Falcon DM Icon" />
</p>

<h1 align="center">Falcon DM</h1>

<p align="center">
  <strong>macOS download manager — multi-thread HTTP, HLS, YouTube, browser capture.</strong><br>
  Tauri v2 (Rust) backend + React/TypeScript UI. Local-only: no telemetry, no cloud queue.
</p>

<p align="center">
  <a href="https://github.com/batu3384/falcon-dm/actions/workflows/ci.yml"><img src="https://github.com/batu3384/falcon-dm/actions/workflows/ci.yml/badge.svg" alt="CI" /></a>
  <a href="https://github.com/batu3384/falcon-dm/actions/workflows/release.yml"><img src="https://github.com/batu3384/falcon-dm/actions/workflows/release.yml/badge.svg" alt="Release" /></a>
  <img src="https://img.shields.io/badge/platforms-macOS%20(Intel%20%2B%20Apple%20Silicon)-lightgrey" alt="Platforms" />
  <img src="https://img.shields.io/badge/license-MIT-blue" alt="License" />
</p>

<br>

<table align="center" border="0" cellpadding="0" cellspacing="0">
  <tr>
    <td width="50%" align="center" valign="middle">
      <img src="assets/mascot.jpg" width="250" alt="Falcon DM Mascot" style="border-radius: 16px;" />
    </td>
    <td width="50%" valign="middle" style="padding-left: 20px;">
      <h3>Meet Falco (The Sniffer)</h3>
      <p>The mascot reflects what the stack actually does:</p>
      <ul>
        <li><strong>Speed:</strong> DNS-pinned HTTP streaming with bounded redirects.</li>
        <li><strong>Precision:</strong> validated resume, HLS segment assembly, native MP4 muxing.</li>
        <li><strong>Capture:</strong> Chrome/Edge MV3 extension → authenticated localhost API.</li>
      </ul>
    </td>
  </tr>
</table>

---

## Features

| Area | What you get |
|------|----------------|
| **HTTP(S)** | Multi-connection range downloads, byte-exact resume after pause/crash/restart, validator-checked remote changes, DNS-pinned streaming, global and per-download speed limits, proxy |
| **HLS (m3u8)** | Parallel MPEG-TS segments with safe resume, playlist fingerprinting, proxy/speed limit, native MP4 muxing including split audio; live streams are rejected |
| **YouTube / DASH** | Fully native: watch URLs, adaptive quality selection (including audio-only M4A), format-selector fallback chains, MP4 muxing, subtitle export — no external tools needed anywhere |
| **Browser extension** | Automatic capture by file type, per-site never-capture rules, video picker, link grabber (batch ≤100), controllable offline fallback |
| **Queue** | Concurrent queue with priorities and position badges, weekly schedule windows, completion actions (sleep or script), automatic retries, keep-awake while active |
| **Desktop UI** | macOS-native shell, batch paste/import, expired-link refresh, optional SHA-256/MD5 verification, inspector, keyboard shortcuts, EN/TR i18n, decimal Finder-style units, release update check |
| **Security** | One-shot 6-digit pairing, token + Origin allowlist, SSRF/DNS-pinning guards, restricted file opening, zero external tools |

## Core architecture

### Download engine

HTTP(S) downloads run in the Rust engine over DNS-pinned connections. The file is preallocated as one hidden `.falcon.part` file and split into large byte ranges; every connection writes at its own offset, idle connections can take over part of the largest remaining range, and per-range progress is saved next to the file (`.falcon.state`). Pause, crash, automatic retry and app restart continue exactly where each connection stopped.

Resume is deliberately conservative. The state stores a strong ETag or Last-Modified validator and uses `If-Range`; without such a validator, or when the remote entity changes, partial bytes are discarded and the transfer restarts rather than mixing versions. Servers that do not support ranges use a single stream and cannot resume. Malformed `206` responses and compressed response bodies are rejected because they make byte-range integrity impossible. Transient failures are retried with backoff up to five times; a run that moves data resets the budget. Finished files move into place without overwriting (`name (1).ext` on a clash). For a single manually added link, **Request options → Checksum** can require SHA-256 or MD5 verification; all engines verify before the move, and a mismatch deletes the unverified temp file.

### Stream pipeline (HLS + YouTube)

- **HLS:** Rust downloads plain MPEG-TS segments concurrently (proxy + speed limit from Settings). Each segment is written to `*.tmp`, renamed only when complete, and trusted across restarts only with a matching byte-length sidecar. A playlist fingerprint invalidates stale cache when the cut changes while allowing signed-query rotation. fMP4, byte-range, discontinuity and split-audio variants are remuxed to MP4 natively; live playlists fail with `ERR_HLS_LIVE` instead of producing a short partial file.
- **YouTube:** CDN (<code>googlevideo</code>) URLs are rewritten to watch URLs where possible. Extraction (Innertube player API), format selection, multi-connection media download, adaptive audio/video muxing and subtitle export all run in Rust — there is no external tool in the path. The selector understands the `bestvideo`/`bestaudio`/`best`/`bv*`/`ba` grammar with `[height≤N]`/`[ext=…]` filters, `+` merge pairing and `/` fallback chains; an explicit height cap is strict chain-wide, so a 720p choice never comes back as 1080p. Merges are MP4-only: a pair that can only resolve to WebM/Opus input falls through to the next chain entry (typically progressive MP4) instead of failing. Signed media URLs are re-extracted once per run when the CDN rejects them mid-download. Global **proxy** and **speed limit** apply; cookies captured by the browser extension are forwarded to the extraction and the CDN.
- **DASH (mpd):** native manifest parsing, representation selection and parallel segment download with the same resume rules as HLS.

### Queue, schedule and completion

`QueueManager` ticks every 500ms, bounds concurrency with atomics, and cancels in-flight jobs via `tokio::sync::watch`. State lives in SQLite (`rusqlite`, WAL mode) with crash-safe recovery. The scheduler supports a start/stop window and Monday–Sunday toggles; overnight windows belong to their start day. Outside the window workers flush resume state and return to the queue.

When active work drains and no queued or paused rows remain, Falcon DM can do nothing, put the Mac to sleep, or run an executable script once. Failed downloads count as finished for this action. Scripts run directly, not through a shell, and must live in the user's home folder, `/opt/homebrew/bin` or `/usr/local/bin`. On macOS the app holds a `caffeinate` idle-sleep assertion only while workers are active.

### Local API (browser extension and CLI)

Axum serves `http://127.0.0.1:14201`:

| Endpoint | Purpose |
|----------|---------|
| `GET /api/health` | Liveness (loopback, no token, `protocol: 3`) |
| `POST /api/pair` | Extension pairing by 6-digit code (`200` token or `202` pending approval) |
| `POST /api/ping` | Authenticated liveness for a paired extension |
| `GET /api/downloads` | Last 8 live/recent rows for the popup |
| `GET /api/downloads/all` | Filtered, paginated automation rows (`include=url` opt-in) |
| `GET /api/stats` | Queue counters, current speed and disk/limit state |
| `POST /api/intercept` | Media / captured browser download |
| `POST /api/add` | Direct URL (for example grabber batch, context menu or CLI) |
| `POST /api/upload` | Blob media (32 MiB binary cap) |
| `POST /api/pause`, `resume`, `remove` | Per-download queue control |
| `POST /api/pause_all`, `resume_all` | Whole-queue control |
| `POST /api/speed_limit` | Global or per-download speed cap |

Extension requests require `X-Falcon-Token` and a paired `chrome-extension://` or `edge-extension://` Origin. Scripts use the same token with `X-Falcon-Client: cli` and no Origin header; browsers cannot produce that combination cross-origin because the custom header is not CORS-allowed. Non-health API calls are rate-limited; uploads have an additional stricter bucket. Pairing approval is consumed after one token handoff, so re-pairing requires user approval again. See [extension/README.md](extension/README.md).

`python3 scripts/falconctl.py` is a stdlib-only CLI for add/list/pause/resume/remove/limits/stats automation. It reads the token from the same-user `settings.json`; `--wake` can launch the app through `falcondm://wake` when the API is not listening yet.

## Build requirements

- macOS 13.0+ (Ventura or later)
- **Node.js 22+** (matches CI; Node 24 can break Vitest/jsdom on GitHub Actions)
- **Rust** stable (`rustup`)
- **Tauri CLI:** `npm install` (devDependency) or `cargo install tauri-cli`

## Compilation & bootstrapping

The app is fully self-contained: every download engine is Rust, there are no
sidecar binaries to provision and no external tools to install.

Signed releases import the Apple certificate into an ephemeral runner keychain
before the Tauri build. If no signing secrets are configured, the workflow
refuses to publish rather than shipping an unsigned release. Release jobs
publish architecture-specific checksums for the app bundles.

```bash
npm install
npm run tauri dev      # development
npm run tauri build    # .app / .dmg (see release.yml for signed builds)
```

### Browser extension

1. Chrome/Edge → `chrome://extensions` → enable **Developer mode** → **Load unpacked** → `extension/`
2. Click the extension icon. It shows a 6-digit code and Falcon DM asks to connect.
3. Check that Falcon DM shows the same code and click **Approve**. Connected extensions are listed (and can be disconnected) in **Settings → Extension**.
4. Extension popup: the **Send browser downloads to Falcon DM** switch controls capture. Options control captured file types, never-captured sites, success notifications, and **Let Chrome download when Falcon DM is closed**. By default a captured download is blocked when the app cannot take it.
5. **Updates:** Settings → **Check for Updates** compares against GitHub Releases (opens release page when newer)
6. **Wake deep link:** `falcondm://wake` only (no download params — avoids token-in-url leaks). Extension wakes app then uses HTTP API. `tauri dev` may not register the URL scheme.

## Development & contributing

Details: [CONTRIBUTING.md](CONTRIBUTING.md). Security issues: [SECURITY.md](SECURITY.md) (no public issues for vulns). Extended docs: [Wiki](https://github.com/batu3384/falcon-dm/wiki) (source in [`wiki/`](wiki/)).

```bash
# Frontend
npm run lint && npm run format:check && npm run test && npm run build
node extension/smoke-test.mjs && node extension/flow-test.mjs

# Backend (src-tauri/)
cargo fmt --all --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
```

CI on `main` (protected branch) runs: Frontend lint/test/build, Rust fmt/clippy/test/build on macOS arm64, and `cargo deny` supply-chain audit. Intel builds are validated in the **Release** workflow (`release.yml`).

## Security posture

- **Extension trust:** the user compares a 6-digit code shown in the extension and in the app and approves it. Approval permits one token handoff; a later token loss, rotation or re-pairing requires approval again. Declined codes stay declined for ten minutes; disconnecting an extension rotates the token. Captured downloads are **blocked by default** when Falcon cannot take them; the browser fallback is opt-in in the extension options.
- **Network boundary:** download URLs are restricted to HTTP(S) and non-public IP/loopback targets are blocked. Every engine — HTTP, HLS, DASH, YouTube extraction and media — revalidates each redirect hop and pins public DNS results. The local API binds `127.0.0.1` only.
- **Local file access:** **Open** and **Show in Finder** are limited to download roots/app data or exact paths recorded in download rows. Completion scripts are validated executables and run without shell interpolation.
- **Data:** no telemetry. Session cookies stay in backend storage, are encrypted at rest where persisted, are never serialized into frontend download payloads, and are cleared on terminal download states.
- **Deep links:** `falcondm://wake` only — enqueue happens over authenticated HTTP, not query strings.

## License

MIT — see [LICENSE](LICENSE).
