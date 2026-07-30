# Task 5: Real-time Progress & Speed Graph

## Goal
Implement real-time progress updates using Tauri's Event system to send download progress, speed, and status from the Rust backend to the React frontend. Implement a simple speed graph (sparkline) in the UI for active downloads.

## Project Context
- Backend (Tauri Rust): `aria2` engine is integrated and database is ready.
- Frontend (React): Core UI layout, Sidebar, and DownloadList components are created with dummy data.

## Requirements

### 1. Rust Backend (Event Emitter)
- We need a mechanism (like a background `tokio::task`) that periodically (e.g., every 500ms) polls the active downloads from `Aria2Engine` and broadcasts an event to the frontend using Tauri's `AppHandle::emit`.
- Event payload should include:
  ```json
  {
    "id": 1,
    "downloaded_size": 1048576,
    "total_size": 104857600,
    "speed": 512000,
    "status": "Downloading",
    "connections": 8
  }
  ```
- Make sure to update the SQLite Database via `Database::update_download_progress` so the progress is persisted.

### 2. React Frontend (Event Listener)
- Use Tauri's `@tauri-apps/api/event` `listen` function in a `useEffect` hook in the main `App` or `DownloadList` component to listen for progress events.
- Update the state of the downloads dynamically so progress bars and speed text update smoothly.

### 3. Speed Graph (Sparkline)
- Create a `SpeedGraph.tsx` React component.
- The component should maintain a small array of the last N speed values (e.g., last 20 seconds).
- Render these values as a simple SVG line graph or bar chart (using vanilla SVG/CSS, no heavy charting libraries unless necessary and lightweight like Recharts, but prefer pure SVG for a sparkline).
- Integrate this graph into the `DownloadItem` component when a download is in the "Downloading" state.

## Testing
- Add a mock download in Rust to simulate progress if a real download cannot be started during development.
- Build and test via `rtk npm run tauri dev` or `rtk cargo check`. Ensure no compilation errors.

## Commit
```bash
rtk git add -A
rtk git commit -m "feat: real-time progress events and speed graph"
```

## Report
Write to: `/Users/batuhanyuksel/Documents/downloadmanager/.superpowers/sdd/implementation_plan/task-5-report.md`
