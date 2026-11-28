# Browser Extension

Chrome / Edge / Firefox MV3 extension in `extension/`. It sends browser downloads and page media to Falcon DM over the local API (`127.0.0.1:14201`).

## Install

### Chrome / Edge

1. `chrome://extensions` (or `edge://extensions`) → enable **Developer mode**
2. **Load unpacked** → choose the `extension/` folder

### Firefox

Firefox 136 or newer is required. Use the signed Firefox package when a release is available:

- **AMO:** install the listed extension normally.
- **Self-distributed:** install the signed `.xpi`; unsigned release Firefox builds reject permanent sideloading.
- **Development:** `about:debugging#/runtime/this-firefox` → **Load Temporary Add-on…** → choose `extension/manifest.json`. Temporary add-ons disappear when Firefox restarts.

The manifest declares Gecko id `falcon@falcondm.app` and `data_collection_permissions.required: ["none"]`. Falcon DM does not send extension data to a remote service; captured URLs and origin-scoped cookies go only to the local app API. Mozilla accounts and signing are distribution concerns outside this repository's CI. Self-distribution can use `web-ext sign --channel=unlisted`, which signs without creating a public AMO listing.

One manifest serves both browser families. Firefox uses the ordered `background.scripts` event page; Chrome 121+ ignores that key and runs the single-file `background-worker.js` service worker. After editing `media-utils.js`, `shared.js`, `pairing.js`, `api.js` or `background.js`, run:

```bash
node extension/build-worker.mjs
```

## Pairing

1. Click the extension icon. It shows a 6-digit code and asks Falcon DM to connect.
2. Falcon DM opens a dialog with the same code. Approve it only if both codes match.
3. The extension receives its API token and the popup switches to **Connected**.

The token handoff is one-shot. If the extension loses its token, or the token is rotated after **Disconnect**, pairing must be approved in Falcon DM again — the app never reissues a token silently. Declined codes stay declined for ten minutes; **Request a New Code** starts over. **Settings → Extension** lists connected extensions and pending requests.

Web pages cannot use this flow: the request must carry a browser-extension Origin (`chrome-extension://`, `edge-extension://` or `moz-extension://`), which browsers never let pages forge, and nothing is paired without the user's click.

Firefox has two ids. `falcon@falcondm.app` is the static Gecko id; network requests use the per-install internal UUID in `moz-extension://<uuid>`. Pairing intentionally uses that UUID because it is what the backend can verify from the Origin header. A Firefox reinstall can generate a different UUID and require a new approval.

## What it does

- **Download capture:** matching browser downloads are cancelled in the browser and sent to Falcon DM. Cookies are attached only for the download's own HTTPS origin (YouTube pages may supply cookies to googlevideo streams). Toggle with **Send browser downloads to Falcon DM**.
- **Capture file types:** the options page lists the extensions captured automatically, separated by commas, spaces or newlines. An empty list captures nothing. The default list covers common video, audio, archive, document, image and installer types.
- **Never-capture sites:** the options page accepts one host per line. Exact hosts match exactly; `*.example.com` or `.example.com` matches `example.com` and its subdomains. Rules are tested against the referring page, final URL and original URL hosts. The blocklist applies only to automatic capture — **Download with Falcon DM** in the context menu still works on those sites.
- **Offline behavior:** if Falcon DM cannot take a captured download, the browser copy is blocked by default. Turning on **Let the browser download when Falcon DM is closed** (options page) starts a replacement browser download instead.
- **Video pages:** on YouTube a floating button opens a quality picker, including audio-only M4A; other pages list detected media (MP4, HLS, DASH, blob). Blob captures are limited to 32 MiB.
- **Link grabber:** **Grab Links on This Page…** (popup or page context menu) sends up to 100 links at once and reports per-item failures.
- **Popup:** connection state, live progress of running downloads, quick actions. Success notifications are opt-in; error notifications are always shown.

## Firefox capture differences

Firefox does not implement `chrome.downloads.onDeterminingFilename`, so it cannot offer Chrome's exact capture point. The extension uses two paths:

1. **Primary — blocking `webRequest`:** document requests (`main_frame`, `sub_frame`, `object`) with `Content-Disposition: attachment` or a non-renderable/downloadable content type are examined after response headers arrive but before Firefox's download manager engages. If the existing capture extension and never-capture rules match, the request is cancelled and the same dedup/hijack/enqueue path used by Chrome hands it to Falcon. Filenames are decoded from both `filename=` and RFC 5987 `filename*=` parameters, falling back to the URL basename. Media-element, XHR and other subresource traffic is deliberately not cancelled.
2. **Fallback — downloads API:** browser-forced downloads that do not look like a download at the HTTP layer (for example some `<a download>` or save-link flows) are caught at `downloads.onCreated`. The extension cancels as early as it can, watches `downloads.onChanged` briefly for filename resolution, enqueues with the server filename when available, then erases the browser row and removes partial/complete browser bytes. If no filename arrives in time it uses the URL basename.

The fallback is inherently weaker than Chrome capture: Firefox may write some bytes before `onCreated` cancellation wins the race. Cleanup is best-effort, not a hard guarantee. Fail-closed therefore means “Falcon blocks and cleans the browser copy best-effort” on Firefox; the optional browser fallback remains explicitly opt-in.

## Other Firefox differences

- The first `falcondm://wake` launch can show Firefox's per-origin external-protocol permission prompt. After approval, waking the app behaves like Chromium.
- Capture cookies come from the browser cookie jar at request time and are sent only to the local app. Separately, Falcon DM's optional yt-dlp browser-cookie setting currently invokes `--cookies-from-browser chrome`; it is Chrome-specific even when the extension is installed in Firefox. Firefox users normally rely on the cookies already supplied by the extension capture path.

## Permissions

`downloads`, `webRequest`, `webRequestBlocking` (used only by Firefox's capture path), `cookies`, `contextMenus`, `storage`, `scripting`, `notifications`, and host access for media detection plus `http://127.0.0.1:14201/*`. No native messaging, clipboard or alarm permissions. Chromium MV3 does not grant blocking webRequest to ordinary extensions; the Chromium service worker never registers that listener.

## Tests

```bash
node extension/smoke-test.mjs   # static contracts, manifest and worker-bundle freshness
node extension/flow-test.mjs    # popup, pairing, overlay, Chrome capture and Firefox-simulated capture
```
