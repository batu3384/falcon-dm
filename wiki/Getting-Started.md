# Getting Started

## Requirements

- macOS 13.0+ (Ventura or later)
- Node.js 22+ (matches CI)
- Rust stable (`rustup`)
- Tauri CLI via `npm install` or `cargo install tauri-cli`
- Homebrew: `brew install aria2 ffmpeg yt-dlp`

## Provision sidecars

Sidecars are not committed. Run once:

```bash
./scripts/provision-sidecars.sh
```

This copies `aria2c` and `ffmpeg` into `src-tauri/binaries/` for your architecture.

## Run in development

```bash
npm install
npm run tauri dev
```

## Build release locally

```bash
npm run tauri build
```

Signed release builds use the GitHub **Release** workflow with Apple signing secrets configured.

## First-time setup

1. Open **Settings** and note your API token (or rotate it).
2. Load the browser extension (see [Browser Extension](Browser-Extension)).
3. Approve the extension ID when a pair request appears.
4. Install the native messaging host (onboarding wizard or Settings → General).
5. Optional network tuning: **Settings → Network** — proxy, speed limit, max connections per server.
6. Optional YouTube: **Settings → General** — yt-dlp path, **Use browser cookies for yt-dlp** (off by default).
7. Optional extension: popup **Block when offline** if you prefer fail-closed hijack.
8. Optional scheduler: toolbar **Scheduler** — start/stop time window (no shutdown/dial-up modes).

## Verify install

```bash
npm run lint && npm test && npm run build
node extension/smoke-test.mjs
cd src-tauri && cargo clippy --all-targets -- -D warnings && cargo test
```

## Check for updates

In the running app: **Settings → General → Check for updates**. Compares the installed version to the latest GitHub Release and opens the release page when newer.
