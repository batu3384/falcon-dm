# Troubleshooting

## Extension cannot pair

- Confirm Falcon DM is running (`GET http://127.0.0.1:14201/api/health`)
- Install native host manifests and reload the extension
- Approve the exact 32-character extension ID in Settings
- Check token was not reset after pairing (re-pair if needed)

## Downloads stay in browser (not Falcon)

- Pairing or native host failed → extension intentionally falls back to native browser download
- Check extension console and Falcon logs

## HTTP download slow or single connection

- Server may not support byte ranges or file smaller than parallel threshold (~512 KB)
- Check **Settings → Network → Max connections per server**
- Resume downloads always use a single connection (`.falcon.part` present)

## YouTube failures

- Install `yt-dlp`: `brew install yt-dlp`
- Set custom path in Settings if not on PATH
- Use watch URL, not CDN URL

## yt-dlp / ffmpeg / aria2 missing

Run `./scripts/provision-sidecars.sh` or install via Homebrew.

## `tauri dev` and deep links

URL scheme registration may not work in dev mode. Use the running app + HTTP API for extension testing.

## CI / Node version

Use Node 22+. Node 24 can break Vitest/jsdom in CI.
