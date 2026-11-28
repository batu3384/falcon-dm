# Contributing to Falcon DM

Thanks for your interest in contributing! This guide covers the development
setup and the standards a contribution must meet before merge.

## Development Setup

### Prerequisites

- **Node.js** 22+ and npm (matches CI)
- **Rust** stable toolchain (`rustup`)
- **macOS** (Falcon DM desktop target is macOS-only for now)
- **Chrome/Edge** to load the `extension/` folder unpacked

### First run

```bash
git clone https://github.com/batu3384/falcon-dm.git
cd falcon-dm

npm install
npm run tauri dev
```

Load the extension: open `chrome://extensions`, enable Developer Mode,
"Load unpacked", select the `extension/` folder. Click the extension icon — it
shows a 6-digit code — then approve the same code in Falcon DM's pairing dialog
(or **Settings → Extension**).

## Development Workflow

- Branch off `main`: `git checkout -b feat/my-feature`.
- Match the existing commit style: one imperative sentence, capitalized, trailing
  period, focused on the *why* (e.g. `Fix hijack filename race and speed connected enqueue.`).
- Keep commits focused; one logical change per commit.

## Checks Before Pushing

CI enforces all of these; run them locally first:

```bash
# Frontend
npm run lint            # ESLint, --max-warnings 0
npm run format:check    # Prettier
npm run test            # Vitest
npm run build           # tsc + vite build
node extension/smoke-test.mjs   # static contracts (locales, permissions)
node extension/flow-test.mjs    # popup/pairing/overlay flows, mocked chrome API

# Backend (in src-tauri/)
cargo fmt --all --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
```

Format everything with `npm run format` and `cargo fmt` as needed.

## Pull Requests

- Open a PR against `main`.
- Fill in the PR template (change type, test notes, breaking changes).
- Ensure CI is green. A maintainer will review.

## Architecture Pointers

- **Backend** (`src-tauri/src/`): `download/` (native http, hls, dash and
  YouTube engines plus the MP4 muxer, queue, progress, limiter, job),
  `pairing.rs` + `local_api.rs` (extension code
  pairing and the localhost HTTP API), `storage/` (SQLite via r2d2 pool +
  versioned migrations), `util/` (security helpers: SSRF, path traversal,
  DNS pinning, cookie encryption), `lib.rs` (Tauri commands).
- **Frontend** (`src/`): `api/` (typed Tauri IPC wrappers + zod validation),
  `store/` (Zustand: downloads, app, ui, toast), `lib/schema.ts` (zod schemas),
  `components/` (React, macOS-style shell; `components/settings/` holds the
  Settings panes). State flows store→component, never via prop drilling.
- **Extension** (`extension/`): MV3 service worker + content script. Talks to
  the desktop app over the authenticated localhost HTTP API after 6-digit code
  pairing. Supports capture pause, optional browser fallback, and batch
  enqueue (≤100).

Look for `// ponytail:` comments in the codebase — they document non-obvious
decisions and the rationale behind them.
