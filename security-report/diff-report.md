# Diff Security Report — Falcon DM roadmap fixes

**Date:** 2026-07-30  
**Mode:** Incremental (adversarial + targeted security)  
**Verdict after fixes:** CONCERNS → addressed CRITICAL items

## Critical (fixed)

1. **Extension cancel-before-success** (`extension/background.js`)  
   - Was: `suggest({cancel:true})` then POST → data loss on API fail  
   - Now: POST first; success → cancel; fail → keep browser download

2. **Hardcoded API token** (`falcon-dm-local-v1`)  
   - Was: same token every install  
   - Now: UUID on first boot; legacy token rejected; settings.json mode 0600

3. **HLS double-spawn race** (`queue.rs`)  
   - Was: spawn then DB update → dual ffmpeg  
   - Now: claim map + DB `Downloading` before spawn

4. **CORS any-extension + weak token**  
   - Now: first authenticated extension ID pinned to allowlist; subsequent foreign IDs → 403

## Warnings (fixed)

- Transient aria2 RPC errors no longer mark download Failed
- Orphan restore clears all GIDs
- `open_folder` / `open_file` path allowlist
- Resume All excludes Queued
- Clipboard monitor default off
- HLS `Merging` status written to DB
- `is_hls_url()` helper
- HEAD uses cookies/referrer/UA
- MEDIA_URLS cleaned on tab close

## Remaining residual risk (accepted)

- aria2 `--rpc-secret` still on CLI (ps-visible) — needs aria2 secret-file support
- Cookies still plaintext in SQLite while download active
- No automated e2e for Chrome download hijack

## Verification

- `cargo test --lib` → 12 passed  
- `npm run build` → pass (after SettingsModel type fix)
