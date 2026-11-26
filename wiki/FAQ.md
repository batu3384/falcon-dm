# FAQ

## Is Falcon DM IDM for macOS?

It targets similar jobs (multi-connection HTTP, browser capture, queue) but is a smaller local-first app — not full IDM feature parity. Recent gaps closed: byte-exact parallel HTTP resume, HLS/yt-dlp proxy and speed limits, real yt-dlp progress, code pairing, larger link grabber batches.

## Does it phone home?

No telemetry. Queue and settings stay on your machine. **Check for updates** queries GitHub Releases only when you click the button in Settings.

## Which browsers are supported?

Chromium: Chrome and Edge via the bundled MV3 extension. Firefox is planned ([Roadmap](Roadmap)).

## Can I use it without the extension?

Yes. Add URLs manually or via the local API with a valid token.

## Where are files saved?

Default: `~/Downloads` (configurable in Settings). Category subfolders optional. Per-site **download profiles** can override save subdirs.

## What happens when Falcon is offline during a browser download?

If the app is unreachable when the download starts, capture never engages — the browser downloads normally. If capture has already started and the enqueue fails (app quit, token rotated), the download is **blocked by default**; enabling **Let Chrome download when Falcon DM is closed** in the extension options hands it back to the browser. Manual extension actions (context menu, media picker) never cancel anything — they only show an error.

## Does resume use multiple connections?

Yes. Ranged downloads keep a per-segment progress sidecar (`.falcon.state`) next to the hidden `.falcon.part` file, so pause, crash, retry and app restart all continue every connection byte-exactly in parallel. Servers without range support cannot resume at all — a retry there starts at byte 0.

## How do I update?

- **In app:** Settings → **Check for updates** (opens GitHub Release when newer).
- **From source:** pull latest `main` and rebuild.
- **Signed auto-install inside the app** is not shipped yet ([Roadmap](Roadmap)).

## Are browser cookies sent to yt-dlp?

Only if you enable **Settings → Use browser cookies for yt-dlp**. Off by default.
