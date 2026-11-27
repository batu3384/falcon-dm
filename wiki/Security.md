# Security

## Trust boundaries

| Boundary | Control |
|----------|---------|
| Download URLs | Only HTTP(S); loopback, private, link-local, multicast, CGNAT and other non-public IP targets are blocked. Enqueue checks literal hosts without DNS so offline links can queue. Falcon's own HTTP/HLS clients revalidate every redirect hop and pin public DNS results; yt-dlp's initial URL is validated before launch and its later egress is covered by the validating proxy described below unless a user proxy is configured. |
| Local API | `127.0.0.1` only; per-install UUID token compared in constant time; paired extension Origin allowlist. Non-health requests are rate-limited, with a stricter separate bucket for uploads. |
| Extension pairing | User compares a 6-digit code and approves it in the app. Approval hands the token to that exact extension/code pair once only; token loss or rotation requires a fresh approval, never silent reissue. Declined codes stay declined for ten minutes. |
| Extension data | Popup rows omit URLs, paths and headers. Session cookies are not exposed to the frontend and are cleared on terminal download states. Cookies stored in profiles/rows are encrypted at rest. |
| yt-dlp | Input URLs are validated against private and loopback targets before launch. Unless a user proxy is configured, yt-dlp is launched through an ephemeral loopback validating egress proxy and child processes inherit proxy variables; CONNECT and absolute-form requests pass only when the IP literal or every DNS result is public, and outbound connections use those pinned addresses. Proxy startup failure aborts the download rather than falling back to direct egress. A configured user proxy takes precedence and is trusted as the egress boundary. Known releases older than 2023.07.06 are refused. Per-download cookies are passed through an exact-host Netscape cookie file with owner-only permissions, not a forwarded raw header. The URL follows `--`; unsafe format selectors fall back to the default. Cancellation kills the process group, including ffmpeg postprocessors. |
| External tools | `ffmpeg` (bundled or system) and `yt-dlp` are started by Rust with fixed argument lists; the webview has no shell access. |
| Completion scripts | Run directly, never through a shell; must be executable files in the user's home folder, `/opt/homebrew/bin` or `/usr/local/bin`. |
| File opening | **Open** and **Show in Finder** are restricted to the download roots/app data or the exact path recorded in a download row, preventing a compromised webview from launching arbitrary `.command` or `.app` files. |
| Capture fallback | Fail-closed by default (a capture the app cannot take is cancelled); the browser fallback is opt-in in the extension options. |

The loopback API is not a boundary against malware already running as the same local user: such a process can attempt to forge an extension Origin. The token, Origin allowlist and approval flow primarily stop web pages and unpaired extensions.

## Extension cookies

Cookies are accepted only when their source origin is allowed for the download. They are not exposed in frontend download payloads and are cleared on terminal download states. A **Refresh Link…** keeps them only for the same HTTPS origin or the allowed YouTube→googlevideo case and drops them otherwise. yt-dlp reads Chrome cookies only when the user enables **Let yt-dlp use your Chrome sign-in** in Settings.

## Deep links

Only `falcondm://wake` is registered. No download parameters are accepted in deep links; enqueue happens over the authenticated loopback API.

## Updates

**Check for Updates** fetches public GitHub Release metadata when the user clicks the button. No background auto-update or unsigned install.

## Reporting vulnerabilities

Read [SECURITY.md](https://github.com/batu3384/falcon-dm/blob/main/SECURITY.md). Do **not** open public GitHub issues for security reports.
