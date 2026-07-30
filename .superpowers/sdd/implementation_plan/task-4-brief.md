# Task 4: React Frontend — Core UI

## Goal
Build the core layout and styling for Falcon DM using React and Vanilla CSS (no Tailwind). The design must feel like a premium, native macOS application (vibrancy, dark/light mode, sidebar, list views).

## Project Context
- Tauri v2 + React + TypeScript project at `/Users/batuhanyuksel/Documents/downloadmanager`
- Backend commands `add_download`, `pause_download`, etc. exist but we will mainly build the UI shell first.

## Files to Modify/Create
- `src/App.tsx` — Main layout component (Sidebar + Main Content area)
- `src/App.css` or `src/index.css` — Global CSS variables and design tokens for macOS theme
- `src/components/Sidebar.tsx` — Navigation sidebar
- `src/components/Toolbar.tsx` — Top action bar
- `src/components/DownloadList.tsx` — Main list area for downloads
- `src/components/DownloadItem.tsx` — Individual list item
- `src/components/NewDownloadModal.tsx` — Modal to add a new URL

## Design Requirements (CRITICAL)
- **Vanilla CSS Only**: Do not install Tailwind or Bootstrap. Use standard CSS or CSS Modules.
- **macOS Premium Feel**: Use standard Apple colors (system blue, system gray). Use `font-family: -apple-system, BlinkMacSystemFont, "Segoe UI", Roboto, Helvetica, Arial, sans-serif`.
- **Layout**: 
  - Left sidebar (fixed width ~250px) with categories: All Downloads, Downloading, Completed, Video, Music, Documents.
  - Main area with a top toolbar (Add URL, Start All, Pause All, Settings).
  - Main area list view with columns or rich list items (Filename, Progress Bar, Speed, Size, Status).
- **Dark Mode**: Support `@media (prefers-color-scheme: dark)` natively with CSS variables.
- **Glassmorphism**: Use `backdrop-filter: blur(20px)` on the sidebar and toolbar to simulate macOS vibrancy (with semi-transparent background colors).

## Implementation Details
1. Clear the default `App.tsx` and `App.css` from the Vite template.
2. Define CSS variables in `:root` (light) and `@media (prefers-color-scheme: dark) { :root { ... } }` for colors (bg, text, border, accent).
3. Create the layout structure. Use CSS Flexbox/Grid.
4. Add dummy data (mock downloads) in the UI state to visualize the list.
5. Modal state: clicking "Add URL" opens `NewDownloadModal`. The modal should have an input field and "Download" / "Cancel" buttons.

## Testing
- Run `rtk npm run build` to ensure TypeScript compiles without errors.
- Ensure all components are imported and exported correctly.

## Commit
```bash
rtk git add -A
rtk git commit -m "feat: react frontend core layout and macos styling"
```

## Interfaces
- Consumes: Standard Tauri IPC (setup in `invoke` if needed, but primarily UI only for now).
- Produces: A complete React component tree for the app shell and basic CSS.

## Report
Write to: `/Users/batuhanyuksel/Documents/downloadmanager/.superpowers/sdd/implementation_plan/task-4-report.md`
