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

1. Launch Falcon DM. The welcome guide checks the browser extension, `yt-dlp`, and `ffmpeg`.
2. Load `extension/` unpacked in Chrome or Edge (see [Browser Extension](Browser-Extension)).
3. Click the extension icon — it shows a 6-digit code — and approve the same code in Falcon DM's pairing dialog. You never handle the API token yourself. If pairing is repeated later, approval is required again.
4. Optional download tuning: **Settings → Downloads** — connections per file, global speed limit, weekly scheduler, and the action to run when downloads finish.
5. Optional network tuning: **Settings → Network** — proxy, yt-dlp path, **Let yt-dlp use your Chrome sign-in** (off by default).
6. Optional extension behavior: use the extension options to edit captured file types, never-captured sites, success notifications, and **Let Chrome download when Falcon DM is closed** (off by default).

## First-run tour

- **Add downloads:** click **New Download** and paste one link or batch text. URLs may be separated by newlines or by comma/semicolon/whitespace directly before `http(s)`; `#` comment lines are ignored and duplicates removed. **Import…** and dropping a file onto the link field both require a `.txt` or `.csv` filename and scan the first file as mixed text. Non-empty lines with no valid public HTTP(S) link are counted as skipped.
- **Queue:** waiting rows show a 1-based `#N` position badge ordered by priority and then age, matching backend start order. Use the row menu to pause, resume, change queue priority, copy the link, rename/move completed files, archive, or remove rows.
- **YouTube quality:** watch links offer resolution presets and audio-only M4A; the same choice is available in the extension overlay.
- **Integrity check:** for one link, **Request options → Checksum** accepts SHA-256 or MD5 as bare hex or with an algorithm prefix. A mismatch fails the download and deletes the unverified temp file before it reaches the target folder.
- **Expired links:** on a failed or paused row choose **Refresh Link…** and paste a replacement URL for the same content. HTTP resumes byte-exactly when its validator matches; HLS and yt-dlp reuse matching cache/work files where possible.
- **Keyboard:** Space pauses/resumes the selection, Enter opens a completed file, ⌘O opens, ⌘⇧R reveals in Finder, Ctrl+Space opens the row menu, and ⌘A selects all.
- **Speed limit:** the toolbar, Settings and tray checkmark can toggle it. Re-enabling restores the remembered non-zero limit when available, or 1000 KB/s (1 MB/s decimal) otherwise. Units are decimal, like Finder.
- **Window:** closing the window hides Falcon DM to the tray while downloads continue. Click the Dock or tray icon to reopen it; window size and position are remembered. A manual light/dark choice in Settings also forces the native window appearance.
- **Sleep:** Falcon DM keeps the Mac awake only while downloads are active. The completion action can put the Mac to sleep or run an executable script after the queue drains.

## Verify install

```bash
npm run lint && npm test && npm run build
node extension/smoke-test.mjs && node extension/flow-test.mjs
cd src-tauri && cargo clippy --workspace --all-targets -- -D warnings && cargo test --workspace
```

## Check for Updates

In the running app: **Settings → General → Check for Updates**. Compares the installed version to the latest GitHub Release and opens the release page when newer.
