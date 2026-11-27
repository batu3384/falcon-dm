# Browser Extension

Chrome / Edge MV3 extension in `extension/`. It sends browser downloads and page media to Falcon DM over the local API (`127.0.0.1:14201`).

## Install

1. `chrome://extensions` (or `edge://extensions`) → enable **Developer mode**
2. **Load unpacked** → choose the `extension/` folder

## Pairing

1. Click the extension icon. It shows a 6-digit code and asks Falcon DM to connect.
2. Falcon DM opens a dialog with the same code. Approve it only if both codes match.
3. The extension receives its API token and the popup switches to **Connected**.

The token handoff is one-shot. If the extension loses its token, or the token is rotated after **Disconnect**, pairing must be approved in Falcon DM again — the app never reissues a token silently. Declined codes stay declined for ten minutes; **Request a New Code** starts over. **Settings → Extension** lists connected extensions and pending requests.

Web pages cannot use this flow: the request must carry a `chrome-extension://` (or `edge-extension://`) Origin, which browsers never let pages forge, and nothing is paired without the user's click.

## What it does

- **Download capture:** matching browser downloads are cancelled in Chrome and sent to Falcon DM. Cookies are attached only for the download's own HTTPS origin (YouTube pages may supply cookies to googlevideo streams). Toggle with **Send browser downloads to Falcon DM**.
- **Capture file types:** the options page lists the extensions captured automatically, separated by commas, spaces or newlines. An empty list captures nothing. The default list covers common video, audio, archive, document, image and installer types.
- **Never-capture sites:** the options page accepts one host per line. Exact hosts match exactly; `*.example.com` or `.example.com` matches `example.com` and its subdomains. Rules are tested against the referring page, final URL and original URL hosts. The blocklist applies only to automatic capture — **Download with Falcon DM** in the context menu still works on those sites.
- **Offline behavior:** if Falcon DM cannot take a captured download, the browser copy is blocked by default. Turning on **Let Chrome download when Falcon DM is closed** (options page) starts a replacement browser download instead.
- **Video pages:** on YouTube a floating button opens a quality picker, including audio-only M4A; other pages list detected media (MP4, HLS, DASH, blob). Blob captures are limited to 32 MiB.
- **Link grabber:** **Grab Links on This Page…** (popup or page context menu) sends up to 100 links at once and reports per-item failures.
- **Popup:** connection state, live progress of running downloads, quick actions. Success notifications are opt-in; error notifications are always shown.

## Permissions

`downloads`, `webRequest`, `cookies`, `contextMenus`, `storage`, `scripting`, `notifications`, and host access for media detection plus `http://127.0.0.1:14201/*`. No native messaging, clipboard or alarm permissions.

## Tests

```bash
node extension/smoke-test.mjs   # static contracts
node extension/flow-test.mjs    # popup, pairing and overlay flows with a mocked chrome API
```
