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
   ["Report a vulnerability"](https://github.com/batuhanyuksel/downloadmanager/security/advisories/new)
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

- A per-install random token (UUID v4), compared in constant time.
- Origin allow-listing (only `*-extension://` schemes).
- User-explicit extension pairing (no first-wins auto-approval).
- SSRF / DNS-rebinding guards on download URLs.
- Path-traversal protection on save paths and filename sanitization.

Known accepted risks (e.g. the TOCTOU window between URL validation and the
fetch) are documented inline in the Rust source.
