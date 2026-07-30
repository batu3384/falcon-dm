# Task 9: macOS System Integration

## Goal
Enhance Falcon DM with deep macOS system integration to provide a premium user experience: System Tray (Menu Bar) icon with a context menu, Notifications on download completion/failure, and Dock icon bouncing/badging.

## Project Context
- Tauri v2 project at `/Users/batuhanyuksel/Documents/downloadmanager`.
- The Rust backend manages downloads via aria2/SQLite/QueueManager.
- Downloads change statuses from `Downloading` to `Completed` or `Failed`.

## Requirements

### 1. Notifications
- Add `tauri-plugin-notification` to `src-tauri/Cargo.toml` and initialize it in `lib.rs`.
- In the `QueueManager` (or wherever download state changes are handled in Rust), trigger a system notification when a download finishes successfully or fails.
- Notification Title: "Download Complete" or "Download Failed".
- Notification Body: the filename.

### 2. System Tray (Menu Bar)
- Use Tauri v2's native Tray API (`tauri::tray::TrayIconBuilder`).
- Add a tray icon (use a simple dummy PNG icon or the default Tauri icon in `icons/` for now).
- Add a context menu to the tray:
  - "Show Falcon DM" (brings the main window to the front)
  - "Pause All"
  - "Resume All"
  - "Quit"
- Implement the logic for these menu items in Rust.

### 3. Dock Integration (macOS specific)
- When a download completes and the app is in the background, bounce the macOS Dock icon. (You can use Tauri's `Window::request_user_attention` API).
- Optional/Bonus: Set a dock badge with the number of active downloads. If Tauri v2 doesn't support dock badges natively yet without extra plugins, skip the badge, but ensure the bounce works.

## Testing
- Run `rtk cargo check --manifest-path src-tauri/Cargo.toml`.
- You can manually test it by starting a mock download and seeing if the notification pops up when it completes.

## Commit
```bash
rtk git add -A
rtk git commit -m "feat: macos system integration with tray and notifications"
```

## Report
Write to: `/Users/batuhanyuksel/Documents/downloadmanager/.superpowers/sdd/implementation_plan/task-9-report.md`
