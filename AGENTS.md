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
```

`flow-test.mjs` also probes a running app on `127.0.0.1:14201` and skips that part when nothing listens. CI additionally runs `cargo deny check advisories licenses bans sources` (config in `deny.toml`).

## Layout

- `src-tauri/src/download/` — engines: `http.rs` (offset writes, per-range `.state` resume with entity-validator verification, work stealing, malformed-206/content-encoding rejection), `hls.rs` (native MPEG-TS/fMP4 with playlist-fingerprint cache, AES-128, byte ranges, discontinuities and split audio; SAMPLE-AES is `ERR_HLS_DRM`, live is `ERR_HLS_LIVE`), `dash.rs` (native DASH representations), `youtube.rs` (Innertube extraction, cipher resolution), `ytselect.rs` (format-selector subset parser; strict chain-wide height caps, MP4-only merges), `youtube_job.rs` (extract → pick → native download → `mux.rs` MP4 merge → finalize → subtitles; one re-extract on 401/403/410), `mux.rs` (streaming native MP4 mux/remux: TS concat, fMP4, AV merge), `subtitles.rs` (timedtext + HLS VTT→SRT, ASR dedupe, X-TIMESTAMP-MAP), `http_client.rs` (SSRF-pinned client + validating egress proxy), `queue.rs` (non-blocking tick, `StopReason`, retry budget, shutdown/remove claim tombstones), `progress.rs` (2 Hz events, EMA speed, no publish before counts known), `limiter.rs` (global token bucket, decimal rates, full debt repayment), `checksum.rs` (sha256/md5 verify before finalize), `job.rs` (rename/finalize without overwriting, `FinalizeHints`). No external tools: yt-dlp and ffmpeg were removed on purpose; do not reintroduce them.
- `src-tauri/src/pairing.rs` + `local_api.rs` — extension pairing by 6-digit code, API protocol 3. Token handoff is one-shot per approved code: re-pairing always needs fresh user approval, there is no silent reissue.
- `src/store/` — `downloads.ts` (rows, selection, live speeds), `app.ts` (stats, extension, schedule), `ui.ts` (sheets).
- `src/components/` — macOS-style shell; `settings/` holds the Settings panes.
- `extension/` — `pairing.js`, `popup.*`, `options.*`, `content.js` overlay (shadow DOM).

## Conventions

- Downloads table is schema v5+ (`expected_hash`); per-download `speed_limit_kbps` is v6; nullable `subtitle_tracks` JSON is v7 (NULL = default policy, [] = user disabled). Migration chain lives in `storage/database.rs` — always extend it, never edit released migrations.

- Status changes reach the UI only as full rows (`download-updated`); `download-progress` carries counters for live rows. A progress tick must never revive a stopped row.
- Backend error codes (`ERR_DISK_FULL`, `HTTP 403`, …) are localized in `localizeDownloadError` (`src/api/commands.ts`). Add new codes there and to both locale files.
- UI strings live in `src/locales/{en,tr}.json`; `src/i18n.test.ts` enforces parity. Extension strings live in `extension/_locales`, checked by `smoke-test.mjs` (named placeholders like `{code}`, never `$1`).
- Rust tests that need a network use local `TcpListener` fixtures with `allow_loopback_for_tests()`; muxer tests validate structure and sample-exactness in-process.
- aria2 and the native messaging host were removed on purpose; do not reintroduce them.
- Commits carry no tool attribution: never add `Generated with`, `Co-Authored-By`, or similar trailers — the user rejected them explicitly.
- Capture is fail-closed by default: a browser download the app cannot take is cancelled. The browser fallback (`Let Chrome download when Falcon DM is closed`) is opt-in in extension options — docs must describe it this way, not as fail-open.
- Firefox is supported through the same extension package: ordered background scripts + blocking webRequest capture; pairing uses the internal moz-extension UUID.
- `wiki/` is the source of truth for the GitHub Wiki; publish with `./scripts/sync-wiki.sh` after editing. Keep doc claims aligned with the code (endpoints in `lifecycle.rs`, defaults in `settings.rs` and `extension/shared.js`).
