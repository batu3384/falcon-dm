# Task 6: Queue Manager & Scheduler

## Goal
Implement a Queue Manager and a Scheduler in the Rust backend, and expose the UI to manage queues (start/stop queues, reorder items, set schedule times).

## Project Context
- Tauri v2 + React project at `/Users/batuhanyuksel/Documents/downloadmanager`
- `Aria2Engine`, `Database`, and real-time events are fully working.

## Requirements

### 1. Rust Backend (Queue & Scheduler)
- Create `src-tauri/src/download/queue.rs`.
- `QueueManager` should handle:
  - Concurrent download limits (e.g., max 3 active downloads). If 4 are added, 1 stays `Queued`.
  - Priority management (move a download up/down the queue).
  - Main Queue and Sync Queue (just one main queue is fine for now).
- `Scheduler` functionality:
  - Expose Tauri commands to set schedule: `set_schedule(start_time: Option<String>, stop_time: Option<String>, active: bool)`.
  - A background task that checks the current time against the schedule. If it hits `start_time`, it resumes the queue. If it hits `stop_time`, it pauses all active downloads.

### 2. React Frontend (Scheduler UI)
- Create `src/components/SchedulerModal.tsx`.
- Include Time pickers (or native `<input type="time">`) for Start Time and Stop Time, and a toggle switch to enable/disable the schedule.
- Add a "Scheduler" button in the `Toolbar.tsx` that opens this modal.
- In `DownloadList.tsx`, add small controls (up/down arrows) or right-click context menu options to move a download up or down in priority if it's `Queued`.

### 3. Integration
- The queue manager must correctly pause/resume aria2 downloads. E.g., if a download completes, the queue manager automatically starts the next `Queued` download.

## Testing
- Add a test for the scheduler logic (mocking time if possible, or just testing the struct state).
- Run `rtk cargo check --manifest-path src-tauri/Cargo.toml` and `rtk npm run build` to ensure no build errors.

## Commit
```bash
rtk git add -A
rtk git commit -m "feat: queue manager and scheduler implementation"
```

## Report
Write to: `/Users/batuhanyuksel/Documents/downloadmanager/.superpowers/sdd/implementation_plan/task-6-report.md`
