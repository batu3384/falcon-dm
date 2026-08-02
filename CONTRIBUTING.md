# Contributing to Falcon DM

Thanks for your interest in contributing! This guide covers the development
setup and the standards a contribution must meet before merge.

## Development Setup

### Prerequisites

- **Node.js** 20+ and npm
- **Rust** stable toolchain (`rustup`)
- **macOS** (Falcon DM is macOS-only for now)
- **Homebrew** dependencies: `brew install aria2 ffmpeg yt-dlp`
- A **Chrome/Firefox** browser to load the `extension/` folder unpacked

### First run

```bash
git clone <repo>
cd downloadmanager

# Provision sidecar binaries (aria2c + ffmpeg) for your architecture
./scripts/provision-sidecars.sh

npm install
npm run tauri dev
```

Load the extension: open `chrome://extensions`, enable Developer Mode,
"Load unpacked", select the `extension/` folder, then approve the pairing
request in Falcon DM's Settings.

## Development Workflow

- Branch off `main`: `git checkout -b feat/my-feature`.
- Use **conventional commit** messages: `feat:`, `fix:`, `refactor:`, `docs:`,
  `test:`, `chore:`, `perf:`, `ci:`. Example: `fix(hls): correct total_size estimate`.
- Keep commits focused; one logical change per commit.

## Checks Before Pushing

CI enforces all of these; run them locally first:

```bash
# Frontend
npm run lint            # ESLint, --max-warnings 0
npm run format:check    # Prettier
npm run test            # Vitest
npm run build           # tsc + vite build

# Backend (in src-tauri/)
cargo fmt --check
cargo clippy --all-targets -- -D warnings
cargo test
```

Format everything with `npm run format` and `cargo fmt` as needed.

## Pull Requests

- Open a PR against `main`.
- Fill in the PR template (change type, test notes, breaking changes).
- Ensure CI is green. A maintainer will review.

## Architecture Pointers

- **Backend** (`src-tauri/src/`): `download/` (aria2 engine, queue, hls, ytdlp),
  `storage/` (SQLite via r2d2 pool + versioned migrations), `util.rs` (security
  helpers: SSRF, path traversal, header injection), `lib.rs` (Tauri commands +
  localhost HTTP API for the extension).
- **Frontend** (`src/`): `api/` (typed Tauri IPC wrappers + zod validation),
  `store/` (Zustand: downloads, toast), `lib/schema.ts` (zod schemas),
  `components/` (React). State flows store→component, never via prop drilling.
- **Extension** (`extension/`): MV3 service worker + content script. Talks to
  the desktop app over the authenticated localhost HTTP API.

Look for `// ponytail:` comments in the codebase — they document non-obvious
decisions and the rationale behind them.
