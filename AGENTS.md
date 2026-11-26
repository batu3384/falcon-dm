# Falcon DM — notes for contributors and agents

macOS download manager: Tauri v2 (Rust) backend, React 19 + TypeScript UI, Chrome/Edge MV3 extension.

## Verify before you finish

```bash
# Frontend
npm run lint && npm run format:check && npm run test && npm run build
# Rust (run in src-tauri/)
cargo fmt --all --check && cargo clippy --workspace --all-targets -- -D warnings && cargo test --workspace
# Extension
node extension/smoke-test.mjs && node extension/flow-test.mjs
# Release helper
bash scripts/provision-sidecars.test.sh
```

`flow-test.mjs` also probes a running app on `127.0.0.1:14201` and skips that part when nothing listens. CI additionally runs `cargo deny check advisories licenses bans sources` (config in `deny.toml`).

## Layout

- `src-tauri/src/download/` — engines: `http.rs` (offset writes, per-range `.state` resume, work stealing), `hls.rs` (native MPEG-TS; encrypted/fMP4/byte-range/split-audio go to yt-dlp), `ytdlp.rs` (progress templates, stable work dir per id), `queue.rs` (non-blocking tick, `StopReason`, retry budget), `progress.rs` (2 Hz events, EMA speed), `limiter.rs` (global token bucket), `job.rs` (rename/finalize without overwriting), `tools.rs` (finds ffmpeg/yt-dlp with Homebrew paths).
- `src-tauri/src/pairing.rs` + `local_api.rs` — extension pairing by 6-digit code, API protocol 2.
- `src/store/` — `downloads.ts` (rows, selection, live speeds), `app.ts` (stats, extension, schedule), `ui.ts` (sheets).
- `src/components/` — macOS-style shell; `settings/` holds the Settings panes.
- `extension/` — `pairing.js`, `popup.*`, `options.*`, `content.js` overlay (shadow DOM).

## Conventions

- Status changes reach the UI only as full rows (`download-updated`); `download-progress` carries counters for live rows. A progress tick must never revive a stopped row.
- Backend error codes (`ERR_DISK_FULL`, `HTTP 403`, …) are localized in `localizeDownloadError` (`src/api/commands.ts`). Add new codes there and to both locale files.
- UI strings live in `src/locales/{en,tr}.json`; `src/i18n.test.ts` enforces parity. Extension strings live in `extension/_locales`, checked by `smoke-test.mjs` (named placeholders like `{code}`, never `$1`).
- Rust tests that need a network use local `TcpListener` fixtures with `allow_loopback_for_tests()`; the HLS end-to-end test skips without ffmpeg.
- aria2 and the native messaging host were removed on purpose; do not reintroduce them.
- Commits carry no tool attribution: never add `Generated with`, `Co-Authored-By`, or similar trailers — the user rejected them explicitly.
- Capture is fail-closed by default: a browser download the app cannot take is cancelled. The browser fallback (`Let Chrome download when Falcon DM is closed`) is opt-in in extension options — docs must describe it this way, not as fail-open.
- `wiki/` is the source of truth for the GitHub Wiki; publish with `./scripts/sync-wiki.sh` after editing. Keep doc claims aligned with the code (endpoints in `lifecycle.rs`, defaults in `settings.rs` and `extension/shared.js`).
