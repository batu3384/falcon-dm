# FAQ

## Is Falcon DM IDM for macOS?

It targets similar jobs (multi-connection HTTP, browser capture, queue) but is a smaller local-first app — not full IDM feature parity. Recent gaps closed: parallel HTTP resume, HLS/yt-dlp proxy and speed limits, extension fail-closed option, larger link grabber batches.

## Does it phone home?

No telemetry. Queue and settings stay on your machine. **Check for updates** queries GitHub Releases only when you click the button in Settings.

## Which browsers are supported?

Chromium: Chrome and Edge via the bundled MV3 extension. Firefox is planned ([Roadmap](Roadmap)).

## Can I use it without the extension?

Yes. Add URLs manually or via the local API with a valid token.

## Where are files saved?

Default: `~/Downloads` (configurable in Settings). Category subfolders optional. Per-site **download profiles** can override save subdirs.

## Why aria2 if HTTP is native Rust?

aria2 remains for legacy session recovery and edge compatibility; primary HTTP path is Rust.

## What happens when Falcon is offline during a browser download?

**Default (fail-open):** the browser keeps its native download. **Block when offline** (extension popup/options): the hijacked download is cancelled. Manual extension actions (context menu, media picker) only show an error.

## Does resume use multiple connections?

Yes, when the server supports ranges and the remaining bytes in `.falcon.part` are large enough (~512 KB+). Otherwise a single connection resumes.

## How do I update?

- **In app:** Settings → **Check for updates** (opens GitHub Release when newer).
- **From source:** pull latest `main` and rebuild.
- **Signed auto-install inside the app** is not shipped yet ([Roadmap](Roadmap)).

## Are browser cookies sent to yt-dlp?

Only if you enable **Settings → Use browser cookies for yt-dlp**. Off by default.
