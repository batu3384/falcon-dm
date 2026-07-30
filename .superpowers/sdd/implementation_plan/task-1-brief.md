# Task 1: Project Scaffolding & Tauri Setup

## Goal
Initialize a Tauri v2 project with React + TypeScript frontend for "Falcon DM" — a macOS download manager application.

## Files to Create
- `src-tauri/Cargo.toml` — Rust dependencies
- `src-tauri/src/main.rs` — Tauri entry point
- `src-tauri/src/lib.rs` — Tauri commands
- `src-tauri/tauri.conf.json` — Tauri configuration
- `package.json` — Node dependencies
- `vite.config.ts` — Vite config
- `tsconfig.json` — TypeScript config
- `src/main.tsx` — React entry
- `src/App.tsx` — Main app component
- `index.html` — HTML entry

## Steps

### Step 1: Initialize Tauri v2 project
Run `npm create tauri-app@latest` in the project directory. Use template `react-ts`, package manager `npm`.

IMPORTANT: The project root is `/Users/batuhanyuksel/Documents/downloadmanager`. The directory already has a `.git` folder and `.superpowers` folder — those should remain untouched. You may need to initialize in a temp dir and move files, or use the Tauri CLI flags to init in-place.

If `npm create tauri-app@latest` doesn't support initializing in an existing non-empty directory, create a temp dir, scaffold there, then move everything back.

### Step 2: Verify it builds
Run `npm install` then `npm run tauri dev`. It should compile and open a window.

### Step 3: Configure Tauri for Falcon DM
Edit `src-tauri/tauri.conf.json`:
- `productName`: `"Falcon DM"`
- `identifier`: `"com.falcondm.app"`
- `title`: `"Falcon DM"`
- Window: `width: 1100`, `height: 700`, `minWidth: 800`, `minHeight: 500`

### Step 4: Add Rust dependencies to Cargo.toml

Add these to `[dependencies]`:
```toml
serde = { version = "1", features = ["derive"] }
serde_json = "1"
tokio = { version = "1", features = ["full"] }
rusqlite = { version = "0.31", features = ["bundled"] }
reqwest = { version = "0.12", features = ["stream", "json"] }
uuid = { version = "1", features = ["v4"] }
chrono = "0.4"
dirs = "5"
log = "0.4"
env_logger = "0.11"
```

### Step 5: Create basic Tauri command
In `src-tauri/src/lib.rs`, add a `greet` command:
```rust
#[tauri::command]
fn greet(name: &str) -> String {
    format!("Falcon DM karşılıyor: {}", name)
}
```
Register it in the Tauri builder.

### Step 6: Test IPC from React
In `src/App.tsx`, call `invoke("greet", { name: "test" })` and display the result. Verify it works.

### Step 7: Commit
```bash
git add -A
git commit -m "feat: falcon-dm tauri project scaffolding"
```

## Interfaces
- Consumes: nothing (first task)
- Produces: Working Tauri app shell with `invoke()` IPC between React ↔ Rust

## Report
Write your full report to: `/Users/batuhanyuksel/Documents/downloadmanager/.superpowers/sdd/implementation_plan/task-1-report.md`

Include: what you did, commands run, test results, any concerns.
