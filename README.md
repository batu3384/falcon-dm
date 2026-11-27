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
        <li><strong>Precision:</strong> validated resume, HLS segment assembly, <code>ffmpeg</code> merge.</li>
        <li><strong>Capture:</strong> Chrome/Edge MV3 extension → authenticated localhost API.</li>
      </ul>
    </td>
  </tr>
</table>

---

## Features

| Area | What you get |
|------|----------------|
| **HTTP(S)** | Multi-connection range downloads, byte-exact resume after pause/crash/restart, validator-checked remote changes, DNS-pinned streaming, global speed limit, proxy |
| **HLS (m3u8)** | Parallel MPEG-TS segments with safe resume, playlist fingerprinting, proxy/speed limit, ffmpeg mux; complex playlists use yt-dlp; live streams are rejected |
| **YouTube / DASH** | Watch URLs and quality selection (including audio-only M4A) through `yt-dlp`, real progress, resumable work directories, optional browser cookies, custom binary path |
| **Browser extension** | Automatic capture by file type, per-site never-capture rules, video picker, link grabber (batch ≤100), controllable offline fallback |
| **Queue** | Concurrent queue with priorities and position badges, weekly schedule windows, completion actions (sleep or script), automatic retries, keep-awake while active |
| **Desktop UI** | macOS-native shell, batch paste/import, expired-link refresh, optional SHA-256/MD5 verification, inspector, keyboard shortcuts, EN/TR i18n, decimal Finder-style units, release update check |
| **Security** | One-shot 6-digit pairing, token + Origin allowlist, SSRF/DNS-pinning guards, restricted file opening, hardened yt-dlp invocation |

## Core architecture

### Download engine

HTTP(S) downloads run in the Rust engine over DNS-pinned connections. The file is preallocated as one hidden `.falcon.part` file and split into large byte ranges; every connection writes at its own offset, idle connections can take over part of the largest remaining range, and per-range progress is saved next to the file (`.falcon.state`). Pause, crash, automatic retry and app restart continue exactly where each connection stopped.

Resume is deliberately conservative. The state stores a strong ETag or Last-Modified validator and uses `If-Range`; without such a validator, or when the remote entity changes, partial bytes are discarded and the transfer restarts rather than mixing versions. Servers that do not support ranges use a single stream and cannot resume. Malformed `206` responses and compressed response bodies are rejected because they make byte-range integrity impossible. Transient failures are retried with backoff up to five times; a run that moves data resets the budget. Finished files move into place without overwriting (`name (1).ext` on a clash). For a single manually added link, **Request options → Checksum** can require SHA-256 or MD5 verification; all engines verify before the move, and a mismatch deletes the unverified temp file.

### Stream pipeline (HLS + YouTube)

- **HLS:** Rust downloads plain MPEG-TS segments concurrently (proxy + speed limit from Settings). Each segment is written to `*.tmp`, renamed only when complete, and trusted across restarts only with a matching byte-length sidecar. A playlist fingerprint invalidates stale cache when the cut changes while allowing signed-query rotation. `ffmpeg` remuxes to MP4. Encrypted, fMP4, byte-range, discontinuity and split-audio playlists are handed to yt-dlp; live playlists fail with `ERR_HLS_LIVE` instead of producing a short partial file.
- **YouTube / DASH / complex HLS:** CDN (<code>googlevideo</code>) URLs are rewritten to watch URLs where possible; <code>yt-dlp</code> runs as an external tool (PATH or **Settings → Network → yt-dlp location**). Global **proxy** and **speed limit** apply. Browser cookies are **opt-in** (**Settings → Network → Let yt-dlp use your Chrome sign-in**); cookies captured for one download are written to an exact-host Netscape cookie file instead of forwarded as a raw header. yt-dlp older than 2023.07.06 is refused. Not bundled.

### Queue, schedule and completion

`QueueManager` ticks every 500ms, bounds concurrency with atomics, and cancels in-flight jobs via `tokio::sync::watch`. State lives in SQLite (`rusqlite`, WAL mode) with crash-safe recovery. The scheduler supports a start/stop window and Monday–Sunday toggles; overnight windows belong to their start day. Outside the window workers flush resume state and return to the queue.

When active work drains and no queued or paused rows remain, Falcon DM can do nothing, put the Mac to sleep, or run an executable script once. Failed downloads count as finished for this action. Scripts run directly, not through a shell, and must live in the user's home folder, `/opt/homebrew/bin` or `/usr/local/bin`. On macOS the app holds a `caffeinate` idle-sleep assertion only while workers are active.

### Local API (browser extension)

Axum serves `http://127.0.0.1:14201`:

| Endpoint | Purpose |
|----------|---------|
| `GET /api/health` | Liveness (loopback, no token, `protocol: 2`) |
| `POST /api/pair` | Extension pairing by 6-digit code (`200` token or `202` pending approval) |
| `POST /api/ping` | Authenticated liveness for a paired extension |
| `GET /api/downloads` | Live download rows for the popup |
| `POST /api/intercept` | Media / captured browser download |
| `POST /api/add` | Direct URL (for example grabber batch or context menu) |
| `POST /api/upload` | Blob media (32 MiB binary cap) |

Authenticated requests require `X-Falcon-Token` and a paired `chrome-extension://` or `edge-extension://` Origin. Non-health API calls are rate-limited; uploads have an additional stricter bucket. Pairing approval is consumed after one token handoff, so re-pairing requires user approval again. See [extension/README.md](extension/README.md).

## Build requirements

- macOS 13.0+ (Ventura or later)
- **Node.js 22+** (matches CI; Node 24 can break Vitest/jsdom on GitHub Actions)
- **Rust** stable (`rustup`)
- **Tauri CLI:** `npm install` (devDependency) or `cargo install tauri-cli`
- **Homebrew:** `brew install ffmpeg yt-dlp`

## Compilation & bootstrapping

Sidecars are **not** committed (`.gitignore`). Local provisioning is explicitly unsigned/unverified unless hashes are supplied:

```bash
./scripts/provision-sidecars.sh
```

Copies `ffmpeg` (Homebrew on arm64, pinned static build on Intel) into
`src-tauri/binaries/` as `ffmpeg-<triple>`. Idempotent. Remote provisioning
requires `FFMPEG_SHA256` and verifies the extracted binary before installation:

```bash
ARCH=x86_64 \
FFMPEG_URL='https://example.invalid/immutable-ffmpeg.zip' \
FFMPEG_SHA256='<64-hex SHA-256 of extracted ffmpeg>' \
./scripts/provision-sidecars.sh
```

Release workflows set `RELEASE_MODE=1` and require an immutable `FFMPEG_URL`
and `FFMPEG_SHA256`; missing values fail before any binary is used.
Signed releases also import the Apple certificate into an ephemeral runner
keychain before Tauri build. If no signing secrets are configured, the workflow
selects the explicit unsigned fallback.

Never use an unpinned remote URL in release automation. Release jobs also
publish architecture-specific checksums for the app and its sidecar.
Configure repository variables `FFMPEG_URL` and `FFMPEG_SHA256` before creating
an Intel release; missing values intentionally fail the release instead of
installing an unverifiable remote binary.

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
5. YouTube: install `yt-dlp`; optional custom binary path and cookie opt-in in Settings
6. **Updates:** Settings → **Check for Updates** compares against GitHub Releases (opens release page when newer)
7. **Wake deep link:** `falcondm://wake` only (no download params — avoids token-in-url leaks). Extension wakes app then uses HTTP API. `tauri dev` may not register the URL scheme.

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
- **Network boundary:** download URLs are restricted to HTTP(S) and non-public IP/loopback targets are blocked. Falcon's own HTTP/HLS redirect hops are revalidated and pinned to public DNS results; yt-dlp's initial URL is validated before launch, while subsequent fetching is performed by yt-dlp. The local API binds `127.0.0.1` only.
- **External tools:** `ffmpeg` and `yt-dlp` are started by Rust with fixed argument lists; the URL follows `--`. yt-dlp input is validated before launch, known-vulnerable old releases are refused, and cancellation kills its process group including postprocessors.
- **Local file access:** **Open** and **Show in Finder** are limited to download roots/app data or exact paths recorded in download rows. Completion scripts are validated executables and run without shell interpolation.
- **Data:** no telemetry. Session cookies stay in backend storage, are encrypted at rest where persisted, are never serialized into frontend download payloads, and are cleared on terminal download states. yt-dlp uses browser cookies only when explicitly enabled in Settings.
- **Deep links:** `falcondm://wake` only — enqueue happens over authenticated HTTP, not query strings.

## License

MIT — see [LICENSE](LICENSE).
