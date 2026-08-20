# Security

## Trust boundaries

| Boundary | Control |
|----------|---------|
| Download URLs | SSRF block: no loopback, private IP, or non-http(s) schemes |
| DNS | Public addresses resolved and pinned per request hop |
| Local API | `127.0.0.1` only; UUID API token; extension Origin allowlist |
| Extension pairing | User must approve extension ID; native host proof required |
| Shell / sidecars | Tauri allowlist: `aria2c`, `ffmpeg`, `yt-dlp`, native host only |

## Extension cookies

Cookies accepted only when enqueue needs them. Not exposed in frontend download payloads; cleared on terminal download states.

## Deep links

Only `falcondm://wake` is registered. No download parameters in deep links.

## Reporting vulnerabilities

Read [SECURITY.md](https://github.com/batu3384/falcon-dm/blob/main/SECURITY.md). Do **not** open public GitHub issues for security reports.
