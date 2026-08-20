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
5. Optional: set download paths, category folders, and `max connections per server`.

## Verify install

```bash
npm run lint && npm test && npm run build
cd src-tauri && cargo clippy --all-targets -- -D warnings && cargo test
```
