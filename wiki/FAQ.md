# FAQ

## Is Falcon DM IDM for macOS?

It targets similar jobs (multi-connection HTTP, browser capture, queue) but is a smaller local-first app — not full IDM feature parity. Recent gaps closed: byte-exact parallel HTTP resume, weekly scheduling, completion actions, expired-link refresh, batch URL import, HLS/YouTube proxy and speed limits, capture rules, code pairing, and larger link grabber batches.

## Does it phone home?

No telemetry. Queue and settings stay on your machine. **Check for Updates** queries GitHub Releases only when you click the button in Settings.

## Which browsers are supported?

Chromium: Chrome and Edge via the bundled MV3 extension. Firefox is planned ([Roadmap](Roadmap)).

## Can I use it without the extension?

Yes. Add URLs manually, paste batch text into **New Download**, import a `.txt`/`.csv` link list, drop a `.txt` or `.csv` file onto the link field, or use the local API with a valid token.

## Where are files saved?

Default: `~/Downloads` (configurable in Settings). Category subfolders are optional. Per-site **download profiles** can override save subdirs. File and folder sizes use decimal units (1000-based KB/MB/GB), matching Finder.

## What happens when Falcon cannot take a captured browser download?

The browser copy is **blocked by default** once capture has started but enqueueing fails (for example because the app quit or the token was rotated). Enabling **Let Chrome download when Falcon DM is closed** in the extension options starts a replacement browser download instead. Manual extension actions (context menu, media picker) never cancel anything — they only show an error.

## Which downloads are captured automatically?

Only file types in the extension's **Capture file types** list. Sites in **Never capture on these sites** are skipped; `*.example.com` or `.example.com` includes subdomains. Context-menu sends are explicit and bypass the site blocklist.

## Does resume use multiple connections?

Usually, yes. Ranged downloads keep a per-segment progress sidecar (`.falcon.state`) next to the hidden `.falcon.part` file, so pause, crash, retry and app restart continue every connection byte-exactly in parallel.

Two important exceptions:

- Servers without range support use one connection and restart at byte 0 on retry.
- Ranged resume needs a strong ETag or Last-Modified validator. If the server did not provide one, Falcon cannot prove the partial bytes still belong to the same file, so resume restarts from scratch. A changed file also restarts from scratch.

## Can Falcon verify a publisher-supplied checksum?

Yes. For a single manually added link, open **Request options** in **New Download** and enter SHA-256 or MD5 (bare hex or an algorithm prefix). Verification happens before the file moves into place for every engine; a mismatch fails the download and removes the unverified temp data.

## A signed CDN link expired. Can I keep the partial file?

Yes. Select the failed or paused row → **Refresh Link…** and paste a fresh URL for the same content. HTTP downloads resume byte-exactly when their validator still matches and restart if the entity changed. HLS reuse depends on the playlist fingerprint; YouTube reuse depends on matching partial files in its work directory. Cookies survive only for the same HTTPS origin (or the allowed YouTube→googlevideo case) and are dropped otherwise.

## Can the scheduler run only on some days?

Yes. **Settings → Downloads** has a start/stop window and Monday–Sunday toggles. Overnight windows belong to their start day. Outside the window, running downloads requeue and continue later.

## Can Falcon sleep the Mac or run a script when downloads finish?

Yes. **Settings → Downloads → When downloads finish** offers **Do nothing**, **Put Mac to sleep**, and **Run a script**. The action runs once after active work drains and only when no queued or paused downloads remain; failed downloads count as finished. If queued or paused work is present at that moment, the drain is consumed — deleting that work later does not fire the action until new download activity occurs. Scripts must be executable files in your home folder, `/opt/homebrew/bin` or `/usr/local/bin`, and run directly rather than through a shell.

## Does Falcon prevent sleep while downloading?

Yes. On macOS it holds an idle-sleep assertion only while downloads are active and releases it when the queue goes idle.

## What happens when I close the window?

The close button hides Falcon DM to the menu-bar tray; downloads continue. Clicking the Dock icon or tray icon reopens the window. The tray menu includes **Show Falcon DM**, **New Download**, **Pause All**, **Resume All**, a **Speed Limit** checkmark and **Quit**. Quit stops active workers and gives them up to three seconds to persist resume state. Window size and position are remembered.

## Can one download be slower than the rest?

Yes. Select the row and set **Speed limit** in the Inspector (decimal KB/s). Empty or 0 means the row follows the global cap. Both caps apply concurrently — the effective rate is whichever is stricter — and HTTP/HLS rows react live.

## How do I update?

- **In app:** Settings → **Check for Updates** (opens GitHub Release when newer).
- **From source:** pull latest `main` and rebuild.
- **Signed auto-install inside the app** is not shipped yet ([Roadmap](Roadmap)).

## Do YouTube downloads use my browser sign-in?

Capture cookies come from the browser cookie jar at request time and travel with the download row: the YouTube extraction and the googlevideo media requests both use them, never leaving the local machine. Age-restricted or members-only videos therefore work when captured from a signed-in browser.
