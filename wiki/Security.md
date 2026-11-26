# Security

## Trust boundaries

| Boundary | Control |
|----------|---------|
| Download URLs | SSRF block: no loopback, private IP, or non-http(s) schemes |
| DNS | Public addresses resolved and pinned per request hop |
| Local API | `127.0.0.1` only; UUID API token; extension Origin allowlist |
| Extension pairing | User compares a 6-digit code and approves it in the app; declined codes stay declined |
| External tools | `ffmpeg` (bundled) and `yt-dlp` started by Rust with fixed arguments; webview has no shell access |
| Capture fallback | Fail-closed by default (a capture the app cannot take is cancelled); opt-in browser fallback in the extension options |

## Extension cookies

Cookies accepted only when enqueue needs them. Not exposed in frontend download payloads; cleared on terminal download states. yt-dlp receives browser cookies only when the user enables **Use browser cookies for yt-dlp** in Settings.

## Deep links

Only `falcondm://wake` is registered. No download parameters in deep links.

## Updates

**Check for updates** fetches public GitHub Release metadata when the user clicks the button. No background auto-update or unsigned install.

## Reporting vulnerabilities

Read [SECURITY.md](https://github.com/batu3384/falcon-dm/blob/main/SECURITY.md). Do **not** open public GitHub issues for security reports.
