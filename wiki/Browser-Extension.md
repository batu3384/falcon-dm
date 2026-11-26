# Browser Extension

Chrome / Edge MV3 extension in `extension/`. It sends browser downloads and page media to Falcon DM over the local API (`127.0.0.1:14201`).

## Install

1. `chrome://extensions` (or `edge://extensions`) → enable **Developer mode**
2. **Load unpacked** → choose the `extension/` folder

## Pairing

1. Click the extension icon. It shows a 6-digit code and asks Falcon DM to connect.
2. Falcon DM opens a dialog with the same code. Approve it only if both codes match.
3. The extension receives its API token and the popup switches to **Connected**.

Declined codes stay declined for ten minutes; **Request a New Code** starts over. **Settings → Extension** lists connected extensions and pending requests; **Disconnect** removes an extension and rotates the token.

Web pages cannot use this flow: the request must carry a `chrome-extension://` (or `edge-extension://`) Origin, which browsers never let pages forge, and nothing is paired without the user's click.

## What it does

- **Download capture:** browser downloads are cancelled in Chrome and sent to Falcon DM (cookies only for the same HTTPS origin). Toggle with **Send browser downloads to Falcon DM**.
- **Offline fallback:** a capture that cannot reach the app is blocked by default. Turning on **Let Chrome download when Falcon DM is closed** (options page) hands it back to the browser instead. Capture only engages while the app is connected, so an offline app never eats a download it cannot take.
- **Video pages:** on YouTube a floating button opens a quality picker; other pages list detected media (MP4, HLS, DASH, blob).
- **Link grabber:** **Grab Links on This Page…** (popup or page context menu) sends up to 100 links at once.
- **Popup:** connection state, live progress of running downloads, quick actions.

## Permissions

`downloads`, `webRequest`, `cookies`, `contextMenus`, `storage`, `scripting`, `notifications`, and host access for media detection plus `http://127.0.0.1:14201/*`. No native messaging, clipboard or alarm permissions.

## Tests

```bash
node extension/smoke-test.mjs   # static contracts
node extension/flow-test.mjs    # popup, pairing and overlay flows with a mocked chrome API
```
