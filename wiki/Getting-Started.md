# Getting Started

## Requirements

- macOS 13.0+ (Ventura or later)
- Node.js 22+ (matches CI)
- Rust stable (`rustup`)
- Tauri CLI via `npm install` or `cargo install tauri-cli`
- Homebrew: `brew install ffmpeg yt-dlp`

## Provision sidecars

Sidecars are not committed. Run once:

```bash
./scripts/provision-sidecars.sh
```

This copies `ffmpeg` into `src-tauri/binaries/` for your architecture.

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

1. Load `extension/` unpacked in Chrome or Edge (see [Browser Extension](Browser-Extension)).
2. Click the extension icon — it shows a 6-digit code — and approve the same code in Falcon DM's pairing dialog. You never handle the API token yourself; pairing exchanges it.
3. Optional download tuning: **Settings → Downloads** — connections per file, speed limit, scheduler time window (no shutdown/dial-up modes).
4. Optional network tuning: **Settings → Network** — proxy, yt-dlp path, **Use browser cookies for yt-dlp** (off by default).
5. Optional extension fallback: enable **Let Chrome download when Falcon DM is closed** in the extension options if you prefer the browser to keep captured downloads when the app is unreachable (off by default).

## Verify install

```bash
npm run lint && npm test && npm run build
node extension/smoke-test.mjs && node extension/flow-test.mjs
cd src-tauri && cargo clippy --workspace --all-targets -- -D warnings && cargo test --workspace
```

## Check for Updates

In the running app: **Settings → General → Check for Updates**. Compares the installed version to the latest GitHub Release and opens the release page when newer.
