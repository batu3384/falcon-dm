# Dependency Audit — Falcon DM (2026-09-06)

## Frontend (`package.json`)
| Package | Version | Notes |
|---------|---------|-------|
| react / react-dom | ^19.1.0 | Current major |
| @tauri-apps/api | ^2 | Desktop bridge |
| zod | ^4.4.3 | IPC validation |
| i18next | ^26 | i18n |

No known critical CVEs flagged in this audit pass for direct dependencies. Supply chain: npm lockfile should be kept current via routine `npm audit`.

## Backend (`Cargo.toml` highlights)
| Crate | Use |
|-------|-----|
| tauri 2 | Shell |
| axum | Local API |
| rusqlite | Storage |
| tokio | Async runtime |
| reqwest | HTTP client |
| subtle | Constant-time token compare |

External binaries (not Cargo): **yt-dlp**, **ffmpeg**, **aria2** — user/environment supplied; keep updated separately.

## Supply chain concerns
- Extension has `<all_urls>` host permission (required for capture) — expected for download manager
- Google Fonts CDN in `index.html` — privacy/CDN trust (no secret exposure)

## Action
Routine `npm audit` + `cargo audit` in CI recommended; no blocking CVE found in manual review.
