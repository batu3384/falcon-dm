# Falcon DM browser extension

Chrome / Edge (Manifest V3). Sends browser downloads and page media to the Falcon DM desktop app.

## Install

1. Start Falcon DM.
2. `chrome://extensions` → enable **Developer mode** → **Load unpacked** → this folder.
3. Click the extension icon: it shows a 6-digit code. Approve the same code in Falcon DM.

## Files

| File | Role |
| --- | --- |
| `background.js` | Service worker: download capture, context menus, message router |
| `shared.js` | State, capture rules, badge, health checks, content-script injection |
| `pairing.js` | Code pairing with the app (`/api/pair`) |
| `api.js` | Authenticated calls (`/api/add`, `/api/intercept`, `/api/upload`, `/api/downloads`) |
| `media-utils.js` | URL rules, media detection, YouTube quality presets (shared by worker, popup, page) |
| `content.js` | In-page quality picker, link grabber and video button (shadow DOM) |
| `popup.*`, `options.*`, `chrome.css` | Toolbar popup and settings page |

## Behaviour

- Private, loopback and non-HTTP(S) URLs are never sent.
- Cookies are attached only for the download's own HTTPS origin (YouTube cookies only for googlevideo streams).
- The popup switch **Send browser downloads to Falcon DM** controls capture; when off, the browser downloads files itself.
- Options can limit automatic capture by file type and never capture chosen sites; explicit context-menu sends still work on those sites.
- A captured download that cannot reach Falcon DM is blocked by default. Enabling **Let Chrome download when Falcon DM is closed** in the options page hands it back to the browser instead.

## Tests

```bash
node smoke-test.mjs
node flow-test.mjs
```
