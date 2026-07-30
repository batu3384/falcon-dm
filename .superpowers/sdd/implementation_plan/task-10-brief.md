# Task 10: Settings & Preferences

## Goal
Implement a persistent settings manager in Rust and a User Settings Modal in React. The settings should configure the app's behavior (theme, paths, connection limits).

## Project Context
- Tauri v2 + React project at `/Users/batuhanyuksel/Documents/downloadmanager`.
- The Rust backend manages downloads via aria2/SQLite/QueueManager.
- `Aria2Engine` limits and `QueueManager` limits are currently hardcoded.

## Requirements

### 1. Rust Backend (Settings Manager)
- Create `src-tauri/src/settings.rs`.
- Define a `Settings` struct:
  ```rust
  #[derive(Debug, Clone, Serialize, Deserialize)]
  pub struct Settings {
      pub theme: String, // "system", "light", "dark"
      pub default_download_path: String,
      pub max_concurrent_downloads: u32,
      pub max_connections_per_server: u32,
      pub proxy: Option<String>,
  }
  ```
- Implement `Settings::load` and `Settings::save` which reads/writes to `{app_data_dir}/settings.json`. Provide sensible defaults (e.g., `~/Downloads` for path, `3` for concurrent, `16` for connections).
- Expose Tauri commands: `get_settings()` and `save_settings(settings: Settings)`.
- When settings are saved, dynamically apply them: update `QueueManager`'s concurrent limit and `Aria2Engine`'s limits if possible (or require restart for network limits, but try to update dynamically).

### 2. React Frontend (Settings UI)
- Create `src/components/SettingsModal.tsx`.
- The modal should have tabs or sections:
  - **General**: Theme dropdown (System, Light, Dark). Default Download Path with a "Browse" button (use Tauri's dialog API `open` to pick a folder).
  - **Network**: Number inputs/sliders for Max Concurrent Downloads (1-10) and Max Connections Per Server (1-32). Proxy input field.
- Add a "Settings" ⚙️ icon button to the `Toolbar.tsx` to open the modal.
- Apply the theme change dynamically: if Theme is "dark", add a `dark` class to `document.documentElement`, etc.

## Testing
- Run `rtk cargo check --manifest-path src-tauri/Cargo.toml`.
- Run `rtk npm run build`.

## Commit
```bash
rtk git add -A
rtk git commit -m "feat: settings and preferences management"
```

## Report
Write to: `/Users/batuhanyuksel/Documents/downloadmanager/.superpowers/sdd/implementation_plan/task-10-report.md`
