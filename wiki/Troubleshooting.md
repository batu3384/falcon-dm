# Troubleshooting

## Extension cannot pair

- Confirm Falcon DM is running (`GET http://127.0.0.1:14201/api/health`)
- Build/install native host: `cargo build -p falcon-dm-native-host --manifest-path src-tauri/Cargo.toml`
- Install manifests and reload the extension
- Approve the exact 32-character extension ID in Settings
- Check token was not reset after pairing (re-pair if needed)
- If native pairing times out: ensure you use the slim `falcon-dm-native-host` binary (not an AppKit-linked build)

## Downloads stay in browser (not Falcon)

- **Pause** enabled in extension popup → hijack disabled by design
- Pairing or native host failed → default fail-open keeps browser download
- **Block when offline** enabled → hijack failure cancels browser save (expected)
- Check extension console and Falcon logs

## Hijacked download blocked with no browser fallback

- **Block when offline** is on. Disable in extension popup/options to restore fail-open behavior.

## HTTP download slow or single connection

- Server may not support byte ranges or file smaller than parallel threshold (~512 KB)
- Check **Settings → Network → Max connections per server**
- Very small remaining resume bytes fall back to single connection

## HLS stalls or restarts from zero

- Segment temp lives under app data `downloads_temp/hls-{download_id}/`. Retry should skip completed `.ts` files.
- Check proxy/speed limit in **Settings → Network** if CDN throttles

## YouTube failures

- Install `yt-dlp`: `brew install yt-dlp`
- Set custom path in Settings if not on PATH
- Use watch URL, not CDN URL
- If login-required content: try **Use browser cookies for yt-dlp** (opt-in)
- Global proxy/speed limit from Settings apply to yt-dlp

## yt-dlp / ffmpeg / aria2 missing

Run `./scripts/provision-sidecars.sh` or install via Homebrew.

## Scheduler does not shut down Mac / dial-up

Not implemented. Scheduler only gates queue processing inside a daily time window.

## Update check fails

- Needs network access to `api.github.com`
- Pre-1.0: no in-app installer yet — download the release manually from the opened page

## `tauri dev` and deep links

URL scheme registration may not work in dev mode. Use the running app + HTTP API for extension testing.

## CI / Node version

Use Node 22+. Node 24 can break Vitest/jsdom in CI.
