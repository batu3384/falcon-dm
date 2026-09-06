No issues found by sc-sqli. SQLite uses parameterized queries only.
No issues found by sc-cmdi. yt-dlp/ffmpeg invoked with arg arrays, not shell.
No issues found by sc-ssrf. validate_fetch_url_async + resolve_public_addresses on all HTTP hops.
No issues found by sc-path-traversal. sanitize_filename + save path allowlist on open/move.
No issues found by sc-secrets. API token UUID on first boot; no hardcoded production secrets in diff.
No issues found by sc-xss. React textContent; overlay uses esc(); no innerHTML with user URL.
No issues found by sc-auth. Local API token + extension Origin allowlist + rate limit.
No issues found by sc-cors. Extension-only CORS pinning after first authenticated extension.
No issues found by sc-rce. No eval/deserialize of untrusted input in diff.
No issues found by sc-lang-rust. Clippy clean; cookie_url host validation tightened.
No issues found by sc-lang-typescript. Zod IPC validation; ErrorBoundary present.
