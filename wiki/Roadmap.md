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
| Parallel HTTP (range, speed limit, byte verify) | CA-001 closed |
| HTTP parallel **resume** after `.part` | Remaining bytes use range segments when possible |
| HLS proxy, speed limit, segment resume | Temp dir `hls-{download_id}`; skips existing segments |
| yt-dlp proxy, speed limit, cookie opt-in | Settings toggle; cookies off by default |
| Fail-closed capture by default | Browser fallback is opt-in in extension options |
| Link grabber batch limit | 100 URLs per batch |
| Scheduler scope hint | Time window only; no shutdown/dial-up |
| Release update check | Settings → GitHub Releases compare + open release |
| Duplicate URL guard (DB + transaction) | CA-002 closed |
| Code pairing | 6-digit code approved in the app; native messaging host removed |
| aria2 retirement | Rust HTTP engine with per-range resume replaced it; sidecar, scripts and CI provisioning deleted |
| Byte-exact HTTP resume | Offset writes + `.falcon.state` segment bitmap; crash/restart continue every connection in parallel |
| Parallel HTTP integration tests | Loopback TCP fixtures serve ranged 206 responses — CA-008 closed |
| yt-dlp real progress | Output-template parsing, stable work dir per download, pause resumes from yt-dlp `.part` files |
| Desktop UI redesign | macOS-native shell: sheets, inspector, context menus, settings panes |
| Extension UI redesign | Popup states (connected/pending/blocked/offline), live progress, shadow-DOM overlay |
| Wiki + onboarding docs | Source in `wiki/`, sync via `./scripts/sync-wiki.sh` |
| CI (fmt, clippy, prettier, cargo-deny) | GitHub Actions on PR/push |
| Hijack API auth contract | Token + Origin unit tests; flow-test 401 when API up |
| Cookie at-rest encryption | ChaCha20-Poly1305 in SQLite + settings profiles |
| WCAG 2.5.5 44px targets | `--target` token on pointer controls |
| Schedule cancel-timeout | No Queued demote while worker still writing |

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
| Scheduler shutdown-after-complete | **Later** | IDM-style post-queue shutdown |
| Scheduler dial-up mode | **Later** | Connect-on-demand scheduling |

## How to influence the roadmap

1. Open a [GitHub Issue](https://github.com/batu3384/falcon-dm/issues) with the **enhancement** label.
2. Describe the user problem, not the implementation.
3. Security-sensitive items: follow [SECURITY.md](https://github.com/batu3384/falcon-dm/blob/main/SECURITY.md) — no public exploit details.

## Related docs

- [Architecture](Architecture) — current system design
- [Getting Started](Getting-Started) — build and run today
- [Browser Extension](Browser-Extension) — pairing and intercept flow
