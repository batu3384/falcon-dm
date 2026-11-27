// Lenient checksum input → canonical `algo:hex` form. Mirrors the Rust
// parser in src-tauri/src/download/checksum.rs exactly: the sheet validates
// before submit, the backend re-validates at enqueue (trust boundary), and
// the stored value is what finalize verifies against the finished file.

/**
 * Parse lenient user input into canonical `algo:hex` (lowercase hex).
 * Accepted: bare hex of 64 chars (sha256) or 32 chars (md5), or the prefixed
 * forms `sha256:`/`sha-256:`/`md5:` in any case, whitespace tolerated.
 * Returns null when the input is not a checksum at all.
 */
export function canonicalizeChecksum(input: string): string | null {
  const trimmed = input.trim();
  if (!trimmed) return null;
  let algo: 'sha256' | 'md5';
  let hex: string;
  const colon = trimmed.indexOf(':');
  if (colon >= 0) {
    const kind = trimmed.slice(0, colon).trim().toLowerCase();
    if (kind === 'sha256' || kind === 'sha-256') algo = 'sha256';
    else if (kind === 'md5') algo = 'md5';
    else return null;
    hex = trimmed.slice(colon + 1).trim();
  } else if (trimmed.length === 64) {
    algo = 'sha256';
    hex = trimmed;
  } else if (trimmed.length === 32) {
    algo = 'md5';
    hex = trimmed;
  } else {
    return null;
  }
  if (hex.length !== (algo === 'sha256' ? 64 : 32) || !/^[0-9a-fA-F]+$/.test(hex)) {
    return null;
  }
  return `${algo}:${hex.toLowerCase()}`;
}

/** Display label for the algorithm of a canonical `algo:hex` value. */
export function checksumAlgoLabel(canonical: string): string {
  return canonical.startsWith('md5:') ? 'MD5' : 'SHA-256';
}
