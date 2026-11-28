# Falcon Browser Bridge

Manifest V3 extension for **Chrome / Edge / Firefox 136+**. Sends browser downloads, page media, and blob captures to **Falcon DM** through the local API.

## Install

### Chrome / Edge

1. Open `chrome://extensions` (or `edge://extensions`)
2. Enable **Developer mode**
3. **Load unpacked** → select this `extension/` folder

### Firefox

Use the signed Firefox package when a release is available (AMO or a self-distributed signed XPI). For local development:

1. Run `node extension/build-worker.mjs` after editing background source files
2. Open `about:debugging#/runtime/this-firefox`
3. **Load Temporary Add-on…** → select `extension/manifest.json`

Temporary Firefox add-ons are removed when Firefox restarts. Release signing requires a Mozilla account; unlisted/self-distributed packages can be signed with `web-ext sign --channel=unlisted` without a public AMO listing.

## Pairing

1. Click the extension icon. It shows a 6-digit code and asks Falcon DM to connect.
2. Falcon DM opens a dialog with the same code. Approve only if the codes match.
3. The extension receives a token once and shows **Connected**.

The token handoff is one-shot. After token loss, **Disconnect**, token rotation, or a fresh extension install, the user must approve pairing in Falcon DM again; the app does not reissue tokens silently. Declined codes stay declined for ten minutes. **Settings → Extension** in the app lists connected extensions and pending requests.

Firefox pairs with its per-install `moz-extension://` internal UUID (the Origin host), not the static `falcon@falcondm.app` Gecko id. Reinstalling Firefox or the extension can therefore create a new entry that needs a new approval.

## Features

- Captures matching browser downloads and sends them to Falcon DM with HTTPS origin-scoped cookies (YouTube pages can supply cookies to googlevideo streams)
- **Capture file types:** comma/space/newline-separated extension list; empty means automatic capture is disabled. Defaults cover common media, archive, document, image and installer files
- **Never capture on these sites:** one host per line. `example.com` matches exactly; `*.example.com` or `.example.com` matches the base host and subdomains. Rules apply to the referring page, final URL and original URL hosts. Manual context-menu sends still work on blocked sites
- Fail-closed offline behavior by default; optional **Let the browser download when Falcon DM is closed** browser fallback
- YouTube overlay: quality picker (including audio-only M4A) + segmented progress handoff
- Other sites: detected media list (MP4 / HLS / DASH / blob up to 32 MiB)
- **Grab links on this page:** batch extraction, up to 100 URLs, per-item error reporting
- Context menu: download the clicked link with Falcon DM; grab links on the page
- Popup: connection state, live download progress, and quick actions

Firefox uses blocking `webRequest` for response-driven downloads where possible and a downloads-API fallback for browser-forced downloads. Firefox may write a few bytes before that fallback can cancel a download; the partial row/file is removed best-effort after handoff.

## Architecture

| File | Role |
|------|------|
| `manifest.json` | Shared MV3 manifest; Chromium service worker + Firefox event-page scripts |
| `background.js` | Download interception, context menus, pairing/connection state, upload handling |
| `background-worker.js` | Chromium-only concatenated service-worker bundle (build artifact) |
| `build-worker.mjs` | Rebuilds `background-worker.js` from the ordered background sources |
| `pairing.js` | 6-digit pairing, token persistence, auto-pair retry |
| `popup.html` / `popup.js` | Status and actions |
| `options.html` / `options.js` | Capture file types, never-capture sites, fallback and notifications |
| `content.js` / `overlay.css` | YouTube/media overlay UI (shadow DOM) |
| `media-utils.js` | Format parsing, media normalization, shared helpers, browser namespace shim |
| `smoke-test.mjs` | Static contract checks (no browser needed) |
| `flow-test.mjs` | DOM/API flow tests with mocked Chrome and Firefox APIs |

Firefox 136+ chooses `background.scripts` through `preferred_environment`; Chrome 121+ ignores `background.scripts` and runs `background-worker.js`.

## Permissions

`downloads`, `webRequest`, `webRequestBlocking` (Firefox capture), `cookies`, `contextMenus`, `storage`, `scripting`, `notifications`, host access for media detection, and `http://127.0.0.1:14201/*` for the local API. Chromium does not grant blocking webRequest in MV3 and does not use that path.

## API calls

- `GET /api/health` — unauthenticated liveness, expects `protocol: 3`
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
- Automatic capture is fail-closed by default: if Falcon DM cannot enqueue it, the browser download is blocked unless the user enables the browser fallback in Options. Manual media/context-menu actions do not cancel browser downloads — they only report errors.
- Firefox cannot offer Chrome's exact `onDeterminingFilename` guarantee. Blocking response capture prevents the browser download manager from engaging for covered responses; the fallback is best-effort and removes partial browser bytes after cancellation.

## Test

```bash
node extension/smoke-test.mjs
node extension/flow-test.mjs
```
