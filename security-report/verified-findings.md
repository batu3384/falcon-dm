# Verified Findings — Falcon DM (2026-09-06)

## Fixed this session

| ID | Severity | Location | Finding | Status |
|----|----------|----------|---------|--------|
| F-01 | Medium | extension/background.js intercept | googlevideo intercept used wrong cookie_url | **Fixed** — cookieLookupUrl |
| F-02 | Medium | extension/background.js context menu | Same cookie gap on right-click CDN links | **Fixed** |
| F-03 | Medium | extension/shared.js, content.js | Substring youtube/googlevideo spoof | **Fixed** — host-based helpers |
| F-04 | Medium | src-tauri/util/net.rs | Referrer substring in youtube_page_url_for_download | **Fixed** |
| F-05 | Low | src-tauri/lib.rs resolve_media_referer | Substring youtube.com check | **Fixed** |

## Accepted residual (documented)

| ID | Severity | Location | Finding | Rationale |
|----|----------|----------|---------|-----------|
| R-01 | Low | YouTube CDN | Signed URLs expire without playback | Inherent to YouTube; yt-dlp fallback exists |
| R-02 | Info | extension manifest | `<all_urls>` permission | Required for generic download capture |
| R-03 | Low | SQLite | Active cookies plaintext in DB | Local-only; cleared on complete |
| R-04 | Info | index.html | Google Fonts CDN | UI font; no secrets |

## False positives eliminated
- Cookie leak on HTTP redirect — mitigated by `youtube_cdn_capture(current)` (not a finding)
- evil-youtube.com cookie_url — blocked by `is_youtube_host` (not exploitable)

## Confidence
All fixed items verified by: cargo test (91), vitest (73), smoke-test, clippy, eslint.
