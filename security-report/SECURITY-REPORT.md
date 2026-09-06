# SECURITY-REPORT — Falcon DM Full Scan

**Date:** 2026-09-06  
**Scope:** Entire application (Tauri + React + Chrome extension)  
**Pipeline:** Recon → Hunt (focused) → Verify → Report

## Executive summary

**Risk score: Low** (post-fix)

Five-prong review (Bugbot, security review, adversarial, security-check, manual audit) found **no open Critical/High issues**. Medium issues in YouTube cookie routing and host validation were **fixed and tested**.

## Scan statistics
| Phase | Result |
|-------|--------|
| Languages | Rust, TS, JS |
| Entry points | 7 (see architecture.md) |
| Hunt modules | SSRF, auth, cookies, XSS, injection, path traversal — reviewed |
| Verified findings | 5 fixed, 4 accepted residual |

## Findings by severity

### Critical / High
None open.

### Medium (fixed)
1. Intercept + context menu missing `cookieLookupUrl` for googlevideo → HTTP 403 path broken
2. Extension substring host checks (`videoplayback`, `youtube.com` in path) — spoof surface reduced by backend but extension inconsistent

### Low / Info
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
cargo test     → 91 passed
vitest         → 73 passed
smoke-test.mjs → ok
clippy -D      → clean
eslint         → clean
```

## Reports
- [architecture.md](./architecture.md)
- [dependency-audit.md](./dependency-audit.md)
- [verified-findings.md](./verified-findings.md)
