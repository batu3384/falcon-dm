# Task 8 Report

- Status: DONE
- Commits: 63e42e1
- Tests: Added dummy HLS test in hls.rs
- Concerns: We rely on the `ffmpeg` binary being available on the system path for merging to work properly. Depending on the final deployment for macOS, we might want to bundle `ffmpeg` or verify its existence. 

Implemented HLS capturing using `reqwest` and `tokio`, integrated it directly in the `QueueManager`, and updated `Database` to be cloneable and thread-safe.
