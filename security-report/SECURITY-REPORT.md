# SECURITY-REPORT — Falcon DM Full Scan

**Date:** 2026-09-15 (follow-up) / 2026-09-14 / 2026-09-06 (baseline)  
**Scope:** Entire application (Tauri + React + Chrome extension)  
**Pipeline:** Recon → Hunt (focused) → Verify → Report

## Executive summary

**Risk score: Low** (post-fix)

Five-prong review (Bugbot, security review, adversarial, security-check, manual audit) found **no open Critical/High issues**. Medium issues in YouTube cookie routing, host validation, pause persistence, and SW hydrate races were **fixed and tested**.

2026-09-15: CI smoke (F-171), README/manifest (F-172), tray EN/TR (F-173), scheduler hint (F-174), cancel-timeout demote (F-175 / R-07), cookie at-rest encryption (F-176 / R-03), 44px targets (F-177 / R-05), intercept auth contract (F-178 / R-06). Remaining inherent: YouTube signed-URL TTL, `<all_urls>` capture scope.

2026-09-14: additional Medium (pause session-only, hydrate race, YouTube path-regex spoof, `/shorts/` host spoof, blob overwrite, save-dir mkdir-before-allowlist, scheduler timeout status, 256-row skip, user-pause clobber, profile origin default-port, pause/resume event gap, silent save-path fallback, schedule mutex clobber, settings/scheduler save-before-load, pairing 403→offline, fail-open error toast, poll vs pause/Merging) and Low (clipboard loopback, modal/overlay focus, context menu keyboard, confirm id clash, overlay tab trap, describedby, Google Fonts CDN, toolbar scheduler pressed, palette focus restore, log toast spam, load-fail Retry, profile keys, popup grab while offline) closed. HLS ffmpeg sidecar path mismatch (F-109) closed.

## Scan statistics
| Phase | Result |
|-------|--------|
| Languages | Rust, TS, JS |
| Entry points | 7 (see architecture.md) |
| Hunt modules | SSRF, auth, cookies, XSS, injection, path traversal — reviewed |
| Verified findings | F-01…F-178 fixed (F-160 unused); R-01, R-02 inherent |

## Findings by severity

### Critical / High
None open.

### Medium (fixed)
1. Intercept + context menu missing `cookieLookupUrl` for googlevideo → HTTP 403 path broken
2. Extension substring host checks (`videoplayback`, `youtube.com` in path) — spoof surface reduced by backend but extension inconsistent

### Low / Info
- Google Fonts CDN (closed F-131; system stack)
- YouTube signed URL TTL (inherent)
- `<all_urls>` extension scope (required for capture)

## Remediation roadmap

| Phase | Action | Status |
|-------|--------|--------|
| 1 Immediate | Fix cookie lookup on all extension enqueue paths | ✅ Done |
| 2 Short | Host-based validation parity extension ↔ Rust | ✅ Done |
| 3 Medium | CI: rustfmt, clippy, prettier, eslint, vitest, tsc, smoke, cargo-deny | ✅ Done (GitHub Actions) |
| 4 Long | E2E YouTube muxed download test in CI | Optional |

## Verification gates
```
cargo test --lib       → 131 passed
vitest run             → 142 passed
npx tsc --noEmit       → ok
node extension/smoke-test.mjs → ok
node extension/flow-test.mjs  → ok (live Falcon API skipped; intercept 401 covered when listening)
clippy -D warnings     → ok
```

## Reports
- [architecture.md](./architecture.md)
- [dependency-audit.md](./dependency-audit.md)
- [verified-findings.md](./verified-findings.md)
