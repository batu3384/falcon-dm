# Security

## Trust boundaries

| Boundary | Control |
|----------|---------|
| Download URLs | SSRF block: no loopback, private IP, or non-http(s) schemes |
| DNS | Public addresses resolved and pinned per request hop |
| Local API | `127.0.0.1` only; UUID API token; extension Origin allowlist |
| Extension pairing | User must approve extension ID; native host proof required |
| Shell / sidecars | Tauri allowlist: `aria2c`, `ffmpeg`, `yt-dlp`; native host is separate slim binary |
| Hijack fallback | Default fail-open (browser keeps download); optional fail-closed toggle in extension |

## Extension cookies

Cookies accepted only when enqueue needs them. Not exposed in frontend download payloads; cleared on terminal download states. yt-dlp receives browser cookies only when the user enables **Use browser cookies for yt-dlp** in Settings.

## Deep links

Only `falcondm://wake` is registered. No download parameters in deep links.

## Updates

**Check for updates** fetches public GitHub Release metadata when the user clicks the button. No background auto-update or unsigned install.

## Reporting vulnerabilities

Read [SECURITY.md](https://github.com/batu3384/falcon-dm/blob/main/SECURITY.md). Do **not** open public GitHub issues for security reports.
