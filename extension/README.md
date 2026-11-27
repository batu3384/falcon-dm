# Falcon Browser Bridge

Manifest V3 extension for **Chrome / Edge**. Sends browser downloads, page media, and blob captures to **Falcon DM** through the local API.

## Install

1. Open `chrome://extensions` (or `edge://extensions`)
2. Enable **Developer mode**
3. **Load unpacked** → select this `extension/` folder

## Pairing

1. Click the extension icon. It shows a 6-digit code and asks Falcon DM to connect.
2. Falcon DM opens a dialog with the same code. Approve only if the codes match.
3. The extension receives a token once and shows **Connected**.

The token handoff is one-shot. After token loss, **Disconnect**, token rotation, or a fresh extension install, the user must approve pairing in Falcon DM again; the app does not reissue tokens silently. Declined codes stay declined for ten minutes. **Settings → Extension** in the app lists connected extensions and pending requests.

## Features

- Captures matching browser downloads (`chrome.downloads`) and sends them to Falcon DM with HTTPS origin-scoped cookies (YouTube pages can supply cookies to googlevideo streams)
- **Capture file types:** comma/space/newline-separated extension list; empty means automatic capture is disabled. Defaults cover common media, archive, document, image and installer files
- **Never capture on these sites:** one host per line. `example.com` matches exactly; `*.example.com` or `.example.com` matches the base host and subdomains. Rules apply to the referring page, final URL and original URL hosts. Manual context-menu sends still work on blocked sites
- Fail-closed offline behavior by default; optional **Let Chrome download when Falcon DM is closed** browser fallback
- YouTube overlay: quality picker (including audio-only M4A) + segmented progress handoff
- Other sites: detected media list (MP4 / HLS / DASH / blob up to 32 MiB)
- **Grab links on this page:** batch extraction, up to 100 URLs, per-item error reporting
- Context menu: download the clicked link with Falcon DM; grab links on the page
- Popup: connection state, live download progress, and quick actions

## Architecture

| File | Role |
|------|------|
| `manifest.json` | MV3 permissions and content scripts |
| `background.js` | Download interception, context menus, pairing/connection state, upload handling |
| `pairing.js` | 6-digit pairing, token persistence, auto-pair retry |
| `popup.html` / `popup.js` | Status and actions |
| `options.html` / `options.js` | Capture file types, never-capture sites, fallback and notifications |
| `content.js` / `overlay.css` | YouTube/media overlay UI (shadow DOM) |
| `media-utils.js` | Format parsing, media normalization, shared helpers |
| `smoke-test.mjs` | Static contract checks (no browser needed) |
| `flow-test.mjs` | DOM/API flow tests with mocked `chrome` |

## Permissions

`downloads`, `webRequest`, `cookies`, `contextMenus`, `storage`, `scripting`, `notifications`, host access for media detection, and `http://127.0.0.1:14201/*` for the local API.

## API calls

- `GET /api/health` — unauthenticated liveness, expects `protocol: 2`
- `POST /api/pair` — 6-digit pairing (`202` while user approval is pending)
- `POST /api/ping` — authenticated liveness
- `POST /api/intercept` — captured/media download
- `POST /api/add` — direct URL (grabber batch or context menu)
- `POST /api/upload` — blob media (32 MiB binary cap)
- `GET /api/downloads` — live rows for the popup

Authenticated requests require `X-Falcon-Token` plus the extension Origin. `pair` has no token but does require a browser extension Origin. Falcon DM binds `127.0.0.1` only.

## Security notes

- Pairing requires the same 6-digit code and a manual approval click in the desktop app; the resulting token handoff is one-shot.
- Authentication is `X-Falcon-Token` + exact extension Origin, so ordinary web pages cannot use the API.
- Cookies are never sent off-origin; `Referer` follows browser downgrade rules.
- Automatic capture is fail-closed: if Falcon DM cannot enqueue it, the browser download is blocked unless the user enables the browser fallback in Options. Manual media/context-menu actions do not cancel browser downloads — they only report errors.

## Test

```bash
node extension/smoke-test.mjs
node extension/flow-test.mjs
```
