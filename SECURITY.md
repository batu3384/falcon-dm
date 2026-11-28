# Security Policy

## Reporting a vulnerability

Email **support@batuhan.tech** with `Falcon DM security` in the subject. Include affected version/commit, reproduction steps, impact, and proof of concept if available.

Do **not** file public GitHub issues or pull requests for vulnerabilities. We aim to acknowledge reports within 72 hours and coordinate disclosure after a fix is available.

## Supported scope

Security fixes target the latest commit on `main` and the newest signed macOS release. Falcon DM currently supports macOS 13+ on Intel and Apple Silicon. Linux and Windows builds are not shipped yet.

## Important local boundary

The app intentionally runs an HTTP API on `127.0.0.1:14201` for the browser extension. Authenticated requests must carry the per-install token and a paired `chrome-extension://` or `edge-extension://` Origin. Pairing uses a 6-digit code shown in both places and requires a click in Falcon DM. Approval permits **one** token handoff; token loss, rotation or re-pairing requires approval again. Disconnecting rotates the token. Non-health API calls are rate-limited, with stricter upload limits.

Known residual risk: another process already running on the same user account can attempt loopback requests. Origin checks block ordinary web pages, but local malware is outside this defense in depth boundary.

## Download/network hardening

- URLs are limited to HTTP(S); private, loopback, link-local, multicast, CGNAT and other non-public targets are rejected.
- Every engine — HTTP, HLS, DASH and the YouTube extraction/media clients — revalidates redirect hops and pins public DNS results to reduce rebinding risk. There are no external tools in any download path.
- Resume uses strong entity validators; validator-less or changed remote content restarts rather than mixing partial bytes.
- Cookies are stored only in backend-controlled storage, encrypted at rest where persisted, withheld from frontend payloads, and cleared on terminal states. Link refresh keeps them only for the same HTTPS origin or the allowed YouTube→googlevideo case.
- YouTube media downloads go out with the canonical watch referrer and the row's capture cookies; the extraction client identity stays consistent between the player API and CDN requests.

## Local file/process hardening

- **Open** and **Show in Finder** accept only download roots/app data or exact paths recorded for downloads.
- Completion scripts execute directly, never through a shell, and must be executable files inside the user's home folder, `/opt/homebrew/bin` or `/usr/local/bin`.
- Browser capture is fail-closed by default; the browser fallback is opt-in in extension options.

## In scope

- Remote exploitation or command injection through downloads, playlists, filenames or extension input
- SSRF/private-network access through URLs, redirects or extraction requests
- Local API authentication, pairing, rate-limit or Origin validation bypass
- Cookie/token disclosure or persistence failures
- Arbitrary local file overwrite, execution through Open/Reveal, or shell injection through completion actions

## Out of scope

- Malware already running with the user's full privileges
- Attacks requiring physical access
- Content downloaded intentionally by the user
- Issues in third-party tools; Falcon DM invokes none (all engines are in-process Rust)
