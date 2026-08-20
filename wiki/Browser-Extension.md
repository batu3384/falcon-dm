# Browser Extension

Chromium MV3 extension in the repo `extension/` folder.

## Load unpacked

1. Chrome/Edge → `chrome://extensions`
2. Enable **Developer mode**
3. **Load unpacked** → select the `extension/` directory
4. Copy the 32-character extension ID

## Native messaging host (required for secure pairing)

The extension obtains a single-use proof from the registered native host before calling `/api/pair`.

### From the app

**Onboarding** or **Settings → General → Install native pairing host** (enter extension ID).

### From the terminal (macOS script)

```bash
cargo build --manifest-path src-tauri/Cargo.toml --bin falcon-dm-native-host
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

## YouTube

Extension sends the **watch URL** and JSON field `format` (yt-dlp selector). Never send raw `googlevideo.com` CDN URLs.

## Wake deep link

`falcondm://wake` only wakes the app. Downloads are enqueued over authenticated HTTP — never via URL query parameters (avoids token leaks).
