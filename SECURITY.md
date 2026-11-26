# Security Policy

## Supported Versions

Falcon DM is pre-1.0 software. Security fixes are applied only to the latest
release on the `main` branch.

| Version | Supported          |
| ------- | ------------------ |
| latest  | :white_check_mark: |
| < latest| :x:                |

## Reporting a Vulnerability

**Please do NOT open a public GitHub issue for security vulnerabilities.**

Instead, report them privately:

1. Open a **private security advisory** via GitHub's
   ["Report a vulnerability"](https://github.com/batu3384/falcon-dm/security/advisories/new)
   tab, OR
2. Email the maintainer directly.

Please include:
- A description of the issue and its potential impact.
- Steps to reproduce, including any proof-of-concept.
- Affected versions/commits.

### Response Timeline

- **Acknowledgement:** within 48 hours.
- **Initial assessment:** within 5 business days.
- **Fix or mitigation:** targeted for the next release; you'll be kept informed
  of progress and credited in the advisory unless you prefer otherwise.

## Threat Model (summary)

Falcon DM runs a **localhost-only** HTTP API (`127.0.0.1:14201`) for the
companion browser extension. It is **not** designed to be exposed to a network.
The API is protected by:

- Extension pairing by a 6-digit code the user compares in both windows;
  the API token leaves the app only after that approval. There is no silent
  or first-wins auto-approval, and declined codes stay declined for ten minutes.
- A per-install random token (UUID v4), issued only after pairing approval
  and compared in constant time.
- Origin allow-listing (only paired `*-extension://` origins).
- SSRF / DNS-rebinding guards on download URLs.
- Path-traversal protection on save paths and filename sanitization.
- The webview has no shell or process permissions. `ffmpeg` and `yt-dlp` are
  started by the Rust engine with fixed argument lists; the URL always follows
  `--`, so it can never become an option.

Known accepted risks (e.g. the TOCTOU window between URL validation and the
fetch) are documented inline in the Rust source. Direct HTTP pause/resume uses
byte-range requests against `.falcon.part` files; large remaining parts may
resume with parallel range segments.

Captured browser downloads are **fail-closed by default**: when Falcon DM
cannot take a hijacked download, the browser copy is cancelled rather than
silently kept. The extension option **Let Chrome download when Falcon DM is
closed** restores the fail-open fallback — a trade-off between data-loss risk
(cancelled save) and unintended fail-open downloads.
