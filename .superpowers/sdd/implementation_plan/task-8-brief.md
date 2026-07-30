# Task 8: HLS/DASH Stream Capture

## Goal
Implement HLS (.m3u8) stream capturing in the Rust backend. When a user adds an m3u8 URL, the app should parse the manifest, download all video segments (.ts) concurrently, and merge them into an .mp4 file using FFmpeg.

## Project Context
- Tauri v2 project at `/Users/batuhanyuksel/Documents/downloadmanager`.
- HTTP API to receive downloads (Task 7) and `QueueManager` (Task 6) are active.

## Requirements

### 1. Rust Backend (HLS Parser)
- Add `m3u8-rs = "5.0"` (or similar parsing crate) to `src-tauri/Cargo.toml`.
- Create `src-tauri/src/download/hls.rs`.
- Function: `process_hls_stream(url: &str, save_path: &str)`
  - Fetch the `.m3u8` manifest using `reqwest`.
  - Parse the manifest. If it's a master playlist, pick the highest quality stream.
  - If it's a media playlist, extract all `.ts` segment URLs. Make sure to resolve relative URLs against the base URL of the manifest.
  - Temporarily save these segments to a `.falcondm-temp` directory.

### 2. Integration with Aria2
- Pass the `.ts` segment URLs to the `Aria2Engine` to download them concurrently.
- You can treat them as a batch or just download them using standard `reqwest` in a `tokio` stream if aria2 is too complex to manage for 1000s of tiny segments. Using `tokio` and `reqwest` for the `.ts` files with a concurrency limit (e.g., 10 at a time) is perfectly acceptable and often cleaner for HLS.

### 3. FFmpeg Merging
- Once all segments are downloaded successfully, use `std::process::Command` to run `ffmpeg`.
- Command: `ffmpeg -i "concat:seg1.ts|seg2.ts|..." -c copy output.mp4` (or write a `segments.txt` file and use `ffmpeg -f concat -safe 0 -i segments.txt -c copy output.mp4`).
- Update the download status to `Merging` during this process, and `Completed` when done.
- Delete the temporary `.ts` files and the `.falcondm-temp` directory upon success.

### 4. UI State
- No new UI components are strictly required, but ensure that the "Merging" state is correctly displayed in the `DownloadList` component (Task 4/5 handled statuses, just make sure `Merging` is supported in `DownloadStatus` enum).

## Testing
- Add a dummy test for HLS parsing if possible.
- Run `rtk cargo check --manifest-path src-tauri/Cargo.toml` to ensure the new HLS module compiles.

## Commit
```bash
rtk git add -A
rtk git commit -m "feat: hls stream capturing and ffmpeg merging"
```

## Report
Write to: `/Users/batuhanyuksel/Documents/downloadmanager/.superpowers/sdd/implementation_plan/task-8-report.md`
