# Browser Extension

Chromium MV3 extension in the repo `extension/` folder.

## Load unpacked

1. Chrome/Edge → `chrome://extensions`
2. Enable **Developer mode**
3. **Load unpacked** → select the `extension/` directory
4. Copy the 32-character extension ID

## Native messaging host (required for secure pairing)

The extension obtains a single-use proof from the registered native host before calling `/api/pair`. The host binary is **`falcon-dm-native-host`** — a slim workspace crate without AppKit/Tauri (Chrome spawn safe).

### From the app

**Onboarding** or **Settings → General → Install native pairing host** (enter extension ID).

### From the terminal (macOS script)

```bash
cargo build --manifest-path src-tauri/Cargo.toml -p falcon-dm-native-host
NATIVE_HOST_BIN="$PWD/src-tauri/target/debug/falcon-dm-native-host" \
CHROME_EXTENSION_ID="<chrome-id>" \
EDGE_EXTENSION_ID="<edge-id>" \
./scripts/install-native-host.sh
```

The in-app installer also writes manifests on **Linux** and **Windows** Chromium profile paths when supported.

## Pairing flow

1. Extension requests native proof → `POST /api/pair`
2. Falcon shows pending ID in Settings
3. User clicks **Approve**
4. Extension receives token and uses `X-Falcon-Token` on `/api/intercept` and `/api/add`

## Download hijack

When the browser starts a file download, the extension can send it to Falcon instead.

| Control | Where | Effect |
|---------|-------|--------|
| **Pause** | Popup | Hijack off — browser downloads normally |
| **Block when offline** | Popup or extension options | If Falcon cannot receive the hijacked download, cancel browser save instead of fail-open fallback (default: fail-open) |

Fail-closed applies to **`chrome.downloads.onDeterminingFilename`** only. Context menu, media overlay, and manual URL sends show an error but do not cancel a browser download (there is none).

## Link grabber

**Grab page links with Falcon** sends up to **100** selected URLs per batch via `/api/add`. Responses include per-item `results` so partial success can be retried.

## YouTube

Extension sends the **watch URL** and JSON field `format` (yt-dlp selector). Never send raw `googlevideo.com` CDN URLs. Optional browser cookies for yt-dlp are controlled in **Falcon Settings** (off by default).

## Wake deep link

`falcondm://wake` only wakes the app. Downloads are enqueued over authenticated HTTP — never via URL query parameters (avoids token leaks).

## More detail

See [extension/README.md](https://github.com/batu3384/falcon-dm/blob/main/extension/README.md) in the repository for API contracts and permission rationale.
