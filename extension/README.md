# Falcon DM browser extension (MV3)

Load unpacked from this folder in Chrome/Edge.

## API notes

- Pair: extension first asks registered `com.falcondm.native` host for a single-use proof, then calls `POST /api/pair` → `200` + token, or `202` pending (approve in Falcon Settings). Extension polls until approved.
- Downloads: `POST /api/intercept` or `/api/add` with `X-Falcon-Token`.
- Requests use bounded timeouts.
- **Fail-open (default):** hijack cancels the browser save first; if Falcon cannot receive it, the extension re-queues the same URL via `chrome.downloads.download`.
- **Fail-closed (opt-in):** enable **Block when offline** in popup or options → hijack stays cancelled and a blocking notification is shown when Falcon is offline.
- **Pause:** popup **Pause** disables hijack entirely (`cancel: false` always).
- Batch enqueue (link grabber) accepts up to **100** items; returns per-item `results`; successful items are removed from retry selection while failed items remain retryable.
- Link grabber scans `<a href>`, `download` attributes, and `<video>/<audio>` `src`/`source` tags (documents, archives, media extensions).
- **DASH (`.mpd`)** manifests route through yt-dlp in the desktop app.
- **Blob media** on the page can be saved via `/api/upload` (max **32 MB**).
- **Clipboard monitor** (popup/options): queues copied `http(s)` URLs about once per minute while connected.
- YouTube quality: send JSON field `format` (yt-dlp `-f` selector). Do **not** put format in the URL. Legacy `#falconfmt=` still accepted server-side as internal storage.
- Origin must be `chrome-extension://<id>` and that id must be allowlisted after Settings approve.
- Native messaging host must be installed for Chrome and Edge. Missing host, timeout, or app rejection keeps native browser downloads intact (unless fail-closed is enabled on hijack).
- Development install:

  ```bash
  cargo build --manifest-path src-tauri/Cargo.toml -p falcon-dm-native-host
  NATIVE_HOST_BIN="$PWD/src-tauri/target/debug/falcon-dm-native-host" \
  CHROME_EXTENSION_ID="<chrome-id>" \
  EDGE_EXTENSION_ID="<edge-id>" \
  ./scripts/install-native-host.sh
  ```

  Release bundles include an architecture-specific native host and checksums.
  Run the installer with that extracted binary path after installing the app.

## YouTube

Desktop app needs `yt-dlp` on PATH (or set path in Falcon Settings). Extension sends watch URL + `format`; never googlevideo CDN URLs. Browser cookies reach yt-dlp only when **Settings → Use browser cookies for yt-dlp** is enabled.

## Permissions

- `<all_urls>` host permission is required to intercept browser downloads and sniff media on any site. There is no narrower scope: the download-hijack + media-overlay feature must work on arbitrary pages. The extension talks only to `127.0.0.1:14201` (Falcon DM) — no other network.
- Content scripts are injected **on demand** via `chrome.scripting.executeScript` (popup open or media sniffed), not via an always-on `content_scripts` entry.
- `nativeMessaging` is required for the app-authenticated pairing proof; the host communicates with Falcon DM through a local 0600 Unix socket.
- `tabs`/`activeTab` were dropped: tab URL/title access is covered by `<all_urls>`, and `scripting` covers on-demand injection.

Session cookies are accepted only on enqueue requests that need them. They are
not returned in download list payloads, not copied into the frontend model, and
are cleared when a download reaches a terminal state.

## Smoke test

```bash
node extension/smoke-test.mjs
```

Asserts media helpers, hijack contracts, fail-closed handlers, and batch limit constant.
