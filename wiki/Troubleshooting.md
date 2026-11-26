# Troubleshooting

## Extension cannot pair

- Confirm Falcon DM is running (`GET http://127.0.0.1:14201/api/health` returns `"protocol": 2`)
- Open the extension popup: it shows a 6-digit code while it waits
- Approve the same code in Falcon DM (dialog, or **Settings → Extension**)
- Declined by mistake? Click **Request a New Code** in the popup

## Downloads stay in browser (not Falcon)

- **Send browser downloads to Falcon DM** is off in the extension popup → capture disabled by design
- Pairing not approved yet or the app is offline → capture never engages, the browser keeps the download
- Check the extension console and Falcon logs

## Captured download blocked with no browser fallback

- Blocking is the default: when capture has already started but Falcon DM cannot take the file, the browser copy is cancelled. Enable **Let Chrome download when Falcon DM is closed** in the extension options page to restore the fail-open fallback.

## HTTP download slow or single connection

- The server may not support byte ranges; then one connection is used and a retry starts at byte 0 (no resume)
- Files below ~2 MB use one connection
- Check **Settings → Downloads → Connections per file** and the speed limit in **Settings → Downloads** or the status bar

## HLS stalls or restarts from zero

- Segment temp lives under app data `downloads_temp/hls-{download_id}.falcondm-temp/`. Completed segments are reused; `*.tmp` leftovers are fetched again.
- Check proxy/speed limit in **Settings → Network** if CDN throttles

## YouTube failures

- Install `yt-dlp`: `brew install yt-dlp`
- Set custom path in Settings if not on PATH
- Use watch URL, not CDN URL
- If login-required content: try **Use browser cookies for yt-dlp** (opt-in)
- Global proxy/speed limit from Settings apply to yt-dlp

## yt-dlp / ffmpeg missing

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
