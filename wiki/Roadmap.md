# Roadmap

Product direction for **Falcon DM**. This page tracks milestones — not a release calendar. Status updates when work lands on `main`.

## Status legend

| Label | Meaning |
|-------|---------|
| **Done** | Shipped on `main` |
| **Now** | Active or next up |
| **Next** | Planned, not started |
| **Later** | Backlog / exploratory |

## Done (recent parity work)

| Item | Notes |
|------|-------|
| Native YouTube pipeline | Innertube extraction, selector subset parser (strict height caps, MP4-only merges), adaptive muxing and subtitle export in Rust — yt-dlp and ffmpeg removed entirely |
| Parallel HTTP (range, speed limit, byte verify) | CA-001 closed |
| Byte-exact HTTP resume | Offset writes + `.falcon.state` segment bitmap; crash/restart continue every connection in parallel |
| Resume validator enforcement | Strong ETag/Last-Modified required; validator-less or changed content restarts from zero |
| HTTP response hardening | Malformed `206` responses and compressed byte streams rejected; canonical URL dedupe |
| Optional checksum verification | SHA-256/MD5 expectations on single manual downloads; verified before the final move and mismatches delete temp data |
| HLS proxy, speed limit, segment resume | Temp dir `hls-{download_id}.falcondm-temp`; length sidecars and playlist fingerprints validate cached segments |
| HLS live-stream rejection | Live/upcoming playlists fail with `ERR_HLS_LIVE` instead of saving a partial replay |
| yt-dlp proxy, speed limit, real progress | Output-template parsing, stable work dir per download, pause resumes from yt-dlp `.part` files |
| yt-dlp hardening | Private/loopback input validation, validating egress proxy, exact-host cookie file, 2023.07.06 version floor, process-group cancellation |
| Fail-closed capture by default | Browser fallback is opt-in in extension options |
| Extension capture rules | File-type list plus per-site never-capture rules with wildcard host support |
| Link grabber batch limit | 100 URLs per batch |
| Batch URL import | Multi-line paste, `.txt`/`.csv` import and drop in New Download; skipped lines counted |
| Refresh Link | Failed/paused rows can replace expired URLs; HTTP state rewriting preserves byte-exact resume |
| Weekly scheduler | Start/stop window plus Monday–Sunday toggles; overnight windows belong to their start day |
| Queue completion actions | Do nothing, put Mac to sleep, or run a validated executable script when the queue drains |
| Keep-awake assertion | `caffeinate` only while downloads are active |
| Decimal units | 1000-based KB/MB/GB across app and extension, matching Finder |
| Queue/UI polish | Queue position badges, row keyboard shortcuts, remembered speed limits in the UI and tray |
| Window and tray behavior | Close hides to tray, Dock click reopens, tray offers New Download/pause/resume/speed-limit/quit, window state persists, native appearance follows theme override |
| Release update check | Settings → GitHub Releases compare + open release |
| Duplicate URL guard (DB + transaction) | CA-002 closed |
| One-shot code pairing | 6-digit approval hands the token over once; re-pairing requires approval again |
| aria2 retirement | Rust HTTP engine with per-range resume replaced it; sidecar, scripts and CI provisioning deleted |
| Parallel HTTP integration tests | Loopback TCP fixtures serve ranged 206 responses — CA-008 closed |
| Desktop UI redesign | macOS-native shell: sheets, inspector, context menus, settings panes |
| Extension UI redesign | Popup states (connected/pending/blocked/offline), live progress, shadow-DOM overlay |
| Wiki + onboarding docs | Source in `wiki/`, sync via `./scripts/sync-wiki.sh` |
| CI (fmt, clippy, prettier, cargo-deny) | GitHub Actions on PR/push |
| Hijack API auth contract | Token + Origin unit tests; flow-test 401 when API up |
| Cookie at-rest encryption | ChaCha20-Poly1305 in SQLite + settings profiles |
| WCAG 2.5.5 44px targets | `--target` token on pointer controls |
| Non-blocking scheduler stop | Outside-window workers flush resume state and requeue; the queue tick never waits |

## Now

| Item | Status | Notes |
|------|--------|-------|
| Linux desktop build + packaging | **Now** | Tauri matrix; validate sidecars on distros |

## Next

| Item | Status | Notes |
|------|--------|-------|
| Windows desktop build + packaging | **Next** | Installer smoke |
| Firefox MV3 extension | **Next** | Pairing + intercept parity with Chromium |

## Later

| Item | Status | Notes |
|------|--------|-------|
| BitTorrent / magnet support | **Later** | Out of scope today; SSRF policy must extend first |
| Cloud sync / remote queue | **Later** | Conflicts with local-only product goal |
| Signed in-app auto-update | **Later** | `tauri-plugin-updater` + CI manifest + pubkey (release check exists today) |
| Shutdown after completion | **Later** | Sleep and run-script exist today; full power-off is not implemented |
| Scheduler dial-up mode | **Later** | Connect-on-demand scheduling |

## How to influence the roadmap

1. Open a [GitHub Issue](https://github.com/batu3384/falcon-dm/issues) with the **enhancement** label.
2. Describe the user problem, not the implementation.
3. Security-sensitive items: follow [SECURITY.md](https://github.com/batu3384/falcon-dm/blob/main/SECURITY.md) — no public exploit details.

## Related docs

- [Architecture](Architecture) — current system design
- [Getting Started](Getting-Started) — build and run today
- [Browser Extension](Browser-Extension) — pairing and intercept flow
