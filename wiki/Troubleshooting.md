# Troubleshooting

## Extension cannot pair

- Confirm Falcon DM is running (`GET http://127.0.0.1:14201/api/health` returns `"protocol": 2`)
- Open the extension popup: it shows a 6-digit code while it waits
- Approve the same code in Falcon DM (dialog, or **Settings → Extension**)
- Declined by mistake? Click **Request a New Code** in the popup
- Already paired before? That is expected after token loss, **Disconnect**, or token rotation: approval is one-shot and must be repeated. There is no silent token reissue.

## Downloads stay in browser (not Falcon)

- **Send browser downloads to Falcon DM** is off in the extension popup or options → capture is disabled by design
- The file extension is not in **Capture file types** in extension options; an empty list captures nothing
- The page or download host is in **Never capture on these sites**. Use the page's **Download with Falcon DM** context-menu item for an explicit send; manual sends bypass the site blocklist
- Pairing is not approved, or the app cannot be reached; check the extension console and Falcon logs

## Captured download blocked with no browser fallback

Blocking is the default: when capture has already started but Falcon DM cannot take the file, the browser copy is cancelled. Enable **Let Chrome download when Falcon DM is closed** in the extension options page to make the browser start a replacement download instead.

## HTTP download slow or single connection

- The server may not support byte ranges; then one connection is used and a retry starts at byte 0 (no resume)
- Small files use fewer connections; each range must be large enough to be useful
- Check **Settings → Downloads → Connections per file** and the speed limit in **Settings → Downloads** or the toolbar

## Paused or failed HTTP download restarts from zero

This is intentional when the server does not provide a strong ETag or Last-Modified validator: Falcon cannot prove that the partial bytes still belong to the same remote file, so it refuses to mix old and new content. A changed validator also causes a clean restart. If a signed link merely expired, use **Refresh Link…** on the failed or paused row before retrying; the same content resumes byte-exactly.

A compressed response is also rejected. `ERR_CONTENT_ENCODING` means the server ignored the engine's `Accept-Encoding: identity` requirement, making byte-range integrity impossible.

`ERR_PARTIAL_RESPONSE` means the server returned a malformed or incomplete `206` response that cannot prove it represents the whole file.

## “The downloaded file does not match the expected checksum”

The optional SHA-256/MD5 check failed. This is fail-closed: the temp file is deleted before anything reaches the target folder, and retrying downloads the file again. Check that the pasted hash came from the exact file/version. The field accepts bare hex or `sha256:`, `sha-256:` and `md5:` prefixes; invalid input is rejected before enqueue.

## HLS stalls or restarts from zero

- Segment temp lives under app data `downloads_temp/hls-{download_id}.falcondm-temp/`. Completed segments are reused only when their recorded byte length matches; `*.tmp` leftovers are fetched again
- If the playlist is re-cut or its segment paths change, the cache fingerprint no longer matches and old segments are discarded. Signed query rotation alone does not invalidate the cache
- Check proxy/speed limit in **Settings → Network** if the CDN throttles

## `ERR_HLS_LIVE` / live streams cannot be downloaded

Falcon refuses live or upcoming streams instead of saving a few seconds and calling the file complete. Wait until the stream ends and the site publishes a replay, then download that replay.

## YouTube failures

- Install `yt-dlp`: `brew install yt-dlp`
- yt-dlp older than 2023.07.06 is refused for security; upgrade with `brew upgrade yt-dlp`
- Set a custom path in Settings if it is not on PATH
- Use a watch URL, not a CDN URL
- If login-required content: try **Let yt-dlp use your Chrome sign-in** (opt-in)
- Global proxy/speed limit from Settings apply to yt-dlp

## yt-dlp / ffmpeg missing

Run `./scripts/provision-sidecars.sh` or install via Homebrew. Falcon searches PATH plus common Homebrew, pip-user and python.org locations.

## Completion action did not run

- It fires only after download activity has occurred and then no active, queued or paused rows remain
- Queued or paused rows mean work remains; failed and completed rows count as done
- It is one-shot per drain, not a repeating action. If queued/paused work suppresses a drain, deleting that work later does not fire the action until new download activity occurs
- **Run a script** requires an executable file inside your home folder, `/opt/homebrew/bin` or `/usr/local/bin`. It runs directly, not through a shell, so shell syntax in the path field is not interpreted

## Scheduler does not run at the expected time

- Check both the time window and the weekday toggles in **Settings → Downloads**
- Overnight windows belong to their start day: for 23:00–07:00, the hours after midnight still use the previous weekday's toggle
- Scheduler gates downloads only. Sleep/script completion is configured separately under **When downloads finish**; dial-up scheduling is not implemented

## The close button did not quit the app

This is intended: the window hides to the menu-bar tray and downloads continue. Click the Dock or tray icon to reopen it. Choose **Quit** in the tray menu to exit.

## Update check fails

- Needs network access to `api.github.com`
- Pre-1.0: no in-app installer yet — download the release manually from the opened page

## `tauri dev` and deep links

URL scheme registration may not work in dev mode. Use the running app + HTTP API for extension testing.

## CI / Node version

Use Node 22+. Node 24 can break Vitest/jsdom in CI.
