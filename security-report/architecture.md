# Architecture Map — Falcon DM (2026-09-06)

## Application type
Desktop app (Tauri 2) + Chrome MV3 extension + localhost REST API bridge.

## Languages
| Language | Role |
|----------|------|
| Rust | Download engine, queue, HTTP/HLS/yt-dlp, SQLite, local API |
| TypeScript/React | Main UI (Tauri webview) |
| JavaScript | Chrome extension (background, content, popup) |

## Frameworks & runtime
- **Tauri 2** + **Vite** + **React 19** + **Zustand**
- **Axum** local HTTP API on `127.0.0.1:14201`
- **SQLite** (rusqlite) for download metadata
- **aria2** RPC for segmented HTTP fallback
- **yt-dlp** subprocess for YouTube watch URLs
- **ffmpeg** for HLS merge

## Entry points
| Surface | Input | Auth |
|---------|-------|------|
| Tauri webview | User UI, Tauri commands | Local process |
| `POST /api/add`, `/api/intercept` | Extension JSON | Bearer token + extension Origin allowlist |
| `GET /api/health` | None | Public |
| Native messaging | Extension pairing proof | Pair consent in app |
| Deep link `falcon-dm://` | URL wake only | No enqueue via URL |
| Chrome `webRequest` | Passive media capture | Extension permissions |
| Chrome `downloads.onDeterminingFilename` | Browser download hijack | Same API token path |

## Trust boundaries
1. **Web page → content script** — untrusted page; only `location.href` + captured URLs
2. **Content → background** — extension messages (same extension)
3. **Background → localhost API** — token + pinned extension ID
4. **API → download workers** — URL SSRF validation, cookie host matching
5. **Workers → filesystem** — sanitized filenames, save path allowlist

## Data flow (YouTube A-path)
1. Content overlay picks muxed `googlevideo.com` URL or watch URL + format
2. Background `cookieLookupUrl` → YouTube watch page for cookies
3. API validates URL + `cookie_url_matches_download`
4. Queue routes `googlevideo` → native HTTP with Referer/Cookie
5. Redirect off CDN strips session headers (`http_client.rs`)

## detected_languages
Rust, TypeScript, JavaScript

## detected_frameworks
Tauri, React, Axum, Chrome MV3

## application_type
Desktop + browser extension hybrid
