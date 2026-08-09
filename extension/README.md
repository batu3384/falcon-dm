# Falcon DM browser extension (MV3)

Load unpacked from this folder in Chrome/Edge.

## API notes

- Pair: extension first asks registered `com.falcondm.native` host for a single-use proof, then calls `POST /api/pair` → `200` + token, or `202` pending (approve in Falcon Settings). Extension polls until approved.
- Downloads: `POST /api/intercept` or `/api/add` with `X-Falcon-Token`.
- YouTube quality: send JSON field `format` (yt-dlp `-f` selector). Do **not** put format in the URL. Legacy `#falconfmt=` still accepted server-side as internal storage.
- Origin must be `chrome-extension://<id>` and that id must be allowlisted after Settings approve.
- Native messaging host must be installed for Chrome and Edge. Missing host, timeout, or app rejection keeps native browser downloads intact.

## YouTube

Desktop app needs `yt-dlp` on PATH (or set path in Falcon Settings). Extension sends watch URL + `format`; never googlevideo CDN URLs.

## Permissions

- `<all_urls>` host permission is required to intercept browser downloads and sniff media on any site. There is no narrower scope: the download-hijack + media-overlay feature must work on arbitrary pages. The extension talks only to `127.0.0.1:14201` (Falcon DM) — no other network.
- Content scripts are injected **on demand** via `chrome.scripting.executeScript` (popup open or media sniffed), not via an always-on `content_scripts` entry.
- `nativeMessaging` is required for the app-authenticated pairing proof; the host communicates with Falcon DM through a local 0600 Unix socket.
- `tabs`/`activeTab` were dropped: tab URL/title access is covered by `<all_urls>`, and `scripting` covers on-demand injection.
