# Task 11: Polish, Theme & Animations

## Goal
Transform the core UI into a premium, polished macOS application. This involves configuring Tauri for a modern "frameless" look (transparent titlebar), implementing smooth micro-animations, and refining the typography/spacing.

## Project Context
- Tauri v2 + React project at `/Users/batuhanyuksel/Documents/downloadmanager`.
- The core UI works, settings exist, and downloads are functional.

## Requirements

### 1. Tauri Configuration for macOS Window
- Edit `src-tauri/tauri.conf.json`:
  - Under `app.windows[0]`, add:
    ```json
    "transparent": true,
    "titleBarStyle": "Overlay",
    "hiddenTitle": true
    ```
- This configuration makes the window background transparent (allowing CSS backdrop-filter to shine) and removes the native title bar background, leaving only the macOS "traffic lights" (close/minimize/zoom buttons).

### 2. React UI Adjustments
- In `src/components/Toolbar.tsx` (or whichever component sits at the top of the window), add the `data-tauri-drag-region` attribute to the outermost `div` so the user can drag the window from the custom toolbar. Ensure interactive elements (buttons, inputs) inside the toolbar do *not* have this attribute, otherwise they won't be clickable.
- Ensure the toolbar has enough top padding (e.g., `padding-top: 30px` or `env(safe-area-inset-top)`) to prevent overlapping with the macOS traffic lights on the left.

### 3. CSS Animations & Typography
- Edit `src/App.css` (or `index.css`).
- **Typography**: Set the font stack to `-apple-system, BlinkMacSystemFont, "SF Pro", "Inter", sans-serif`. Use font smoothing: `-webkit-font-smoothing: antialiased;`.
- **Transitions**:
  - Add smooth hover transitions to all buttons and list items: `transition: all 0.2s cubic-bezier(0.25, 0.46, 0.45, 0.94);`.
  - Add subtle scaling on button active state: `transform: scale(0.97);`.
- **Modals**: Make the `SettingsModal` and `NewDownloadModal` fade in and scale up slightly when opened (`animation: modalPop 0.3s cubic-bezier(...)`).
- **Scrollbars**: Style the webkit scrollbars to look like native macOS overlay scrollbars (transparent, rounded, visible on hover).

## Testing
- Run `rtk npm run tauri build` or `rtk npm run build`. (To test the window styling perfectly you'd run `tauri dev`, but ensuring it builds is enough for the subagent).
- Run `rtk cargo check --manifest-path src-tauri/Cargo.toml`.

## Commit
```bash
rtk git add -A
rtk git commit -m "feat: premium macos polish with frameless window and animations"
```

## Report
Write to: `/Users/batuhanyuksel/Documents/downloadmanager/.superpowers/sdd/implementation_plan/task-11-report.md`
