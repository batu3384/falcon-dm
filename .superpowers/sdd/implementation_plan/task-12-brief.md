# Task 12: Build & Distribution

## Goal
Finalize the Falcon DM project by configuring the build targets (DMG, App) in Tauri, generating the final bundle (or ensuring it's ready to be generated), and writing a comprehensive `README.md`.

## Project Context
- Tauri v2 + React project at `/Users/batuhanyuksel/Documents/downloadmanager`.
- All features (aria2, HLS, UI, Settings) are complete.

## Requirements

### 1. Tauri Bundle Configuration
- Ensure `src-tauri/tauri.conf.json` has the correct `bundle` config for macOS:
  ```json
  "bundle": {
    "active": true,
    "targets": ["app", "dmg"],
    "icon": [
      "icons/32x32.png",
      "icons/128x128.png",
      "icons/128x128@2x.png",
      "icons/icon.icns",
      "icons/icon.ico"
    ],
    "macOS": {
      "signingIdentity": null
    }
  }
  ```
  *(We set `signingIdentity` to `null` to prevent codesign errors locally unless the user provides an Apple Developer ID).*

### 2. README.md
- Overwrite the default `README.md` in the project root (`/Users/batuhanyuksel/Documents/downloadmanager/README.md`) with a beautiful, professional presentation of Falcon DM.
- Include:
  - Logo/Header (text is fine: `# Falcon DM 🦅`)
  - Description: The ultimate, native macOS download manager.
  - Features: Multi-threaded downloads (aria2), HLS/DASH video capturing, FFmpeg merging, Native macOS UI (frameless, overlay, notifications, tray), Queue Manager.
  - Setup Instructions:
    - Requirements: `rust`, `node`, `aria2c` (in PATH), `ffmpeg` (in PATH).
    - `npm install`
    - `npm run tauri dev`
    - `npm run tauri build`

### 3. Build Verification
- Run `rtk npm run tauri build` to test if the `.dmg` and `.app` are successfully generated in `src-tauri/target/release/bundle/macos/`.
- If the build takes too long or fails solely due to code signing, it's okay to just do `rtk cargo check` and verify the `tauri.conf.json` syntax.

## Testing
- Run `rtk cargo check --manifest-path src-tauri/Cargo.toml`.
- Run `rtk npm run build` (vite build).

## Commit
```bash
rtk git add -A
rtk git commit -m "chore: configure tauri bundle and create readme"
```

## Report
Write to: `/Users/batuhanyuksel/Documents/downloadmanager/.superpowers/sdd/implementation_plan/task-12-report.md`
