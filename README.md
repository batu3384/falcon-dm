# Falcon DM 🦅

The ultimate, native macOS download manager.

## Features
- **Multi-threaded downloads**: Powered by aria2.
- **HLS/DASH video capturing**: Seamlessly download streaming formats.
- **FFmpeg merging**: Automatically merges streams into single playable files.
- **Native macOS UI**: Frameless design, overlay title bar, notifications, and tray integration.
- **Queue Manager**: Efficiently manage and schedule your downloads.

## Setup Instructions

### Requirements
Ensure you have the following installed and available in your `PATH`:
- `rust` (via rustup)
- `node` (via nvm or brew)
- `aria2c`
- `ffmpeg`

### Installation
1. Install dependencies:
   ```bash
   npm install
   ```

2. Run in development mode:
   ```bash
   npm run tauri dev
   ```

3. Build for production:
   ```bash
   npm run tauri build
   ```
