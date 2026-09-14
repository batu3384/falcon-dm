# SECURITY-REPORT — Falcon DM Full Scan

**Date:** 2026-09-14 (follow-up) / 2026-09-06 (baseline)  
**Scope:** Entire application (Tauri + React + Chrome extension)  
**Pipeline:** Recon → Hunt (focused) → Verify → Report

## Executive summary

**Risk score: Low** (post-fix)

Five-prong review (Bugbot, security review, adversarial, security-check, manual audit) found **no open Critical/High issues**. Medium issues in YouTube cookie routing, host validation, pause persistence, and SW hydrate races were **fixed and tested**.

2026-09-14: additional Medium (pause session-only, hydrate race, YouTube path-regex spoof, `/shorts/` host spoof, blob overwrite, save-dir mkdir-before-allowlist, scheduler timeout status, 256-row skip, user-pause clobber, profile origin default-port, pause/resume event gap, silent save-path fallback, schedule mutex clobber, settings/scheduler save-before-load, pairing 403→offline, fail-open error toast, poll vs pause/Merging) and Low (clipboard loopback, modal/overlay focus, context menu keyboard, confirm id clash, overlay tab trap, describedby, Google Fonts CDN, toolbar scheduler pressed, palette focus restore, log toast spam, load-fail Retry, profile keys, popup grab while offline) closed. HLS ffmpeg sidecar path mismatch (F-109) closed. Residual unchanged: YouTube TTL, `<all_urls>`, plaintext cookies in SQLite, live Chrome hijack E2E (API down), schedule cancel-wait Completes Queued (R-07).

## Scan statistics
| Phase | Result |
|-------|--------|
| Languages | Rust, TS, JS |
| Entry points | 7 (see architecture.md) |
| Hunt modules | SSRF, auth, cookies, XSS, injection, path traversal — reviewed |
| Verified findings | F-01…F-170 fixed; R-01…R-03, R-05…R-07 residual |

## Findings by severity

### Critical / High
None open.

### Medium (fixed)
1. Intercept + context menu missing `cookieLookupUrl` for googlevideo → HTTP 403 path broken
2. Extension substring host checks (`videoplayback`, `youtube.com` in path) — spoof surface reduced by backend but extension inconsistent

### Low / Info
- Google Fonts CDN (closed F-131; system stack)
- YouTube signed URL TTL
- `<all_urls>` extension scope
- Plaintext cookies in local SQLite during active download

## Remediation roadmap

| Phase | Action | Status |
|-------|--------|--------|
| 1 Immediate | Fix cookie lookup on all extension enqueue paths | ✅ Done |
| 2 Short | Host-based validation parity extension ↔ Rust | ✅ Done |
| 3 Medium | CI: `cargo audit`, `npm audit`, extension smoke | Recommended |
| 4 Long | E2E YouTube muxed download test in CI | Optional |

## Verification gates
```
cargo test --lib       → 123 passed
vitest run             → 142 passed
npx tsc --noEmit       → ok
node extension/smoke-test.mjs → ok
node extension/flow-test.mjs  → ok (live Falcon API skipped)
curl 127.0.0.1:14201   → connection refused (R-06)
```

## Reports
- [architecture.md](./architecture.md)
- [dependency-audit.md](./dependency-audit.md)
- [verified-findings.md](./verified-findings.md)
