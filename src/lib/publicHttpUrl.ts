function isBlockedIpv4Octets(o: number[]): boolean {
  if (o[0] === 0) return true;
  if (o[0] >= 224) return true;
  if (o[0] === 127) return true;
  if (o[0] === 10) return true;
  if (o[0] === 192 && o[1] === 168) return true;
  if (o[0] === 169 && o[1] === 254) return true;
  if (o[0] === 172 && o[1] >= 16 && o[1] <= 31) return true;
  if (o[0] === 100 && o[1] >= 64 && o[1] <= 127) return true;
  return false;
}

function isBlockedIpv6Literal(h: string): boolean {
  const v6 = h.split('%')[0];
  if (v6 === '::1' || v6 === '::' || v6 === '0:0:0:0:0:0:0:1' || v6 === '0:0:0:0:0:0:0:0') {
    return true;
  }
  const mapped = v6.match(/:ffff:(\d+\.\d+\.\d+\.\d+)$/i);
  if (mapped) {
    const p = mapped[1].split('.').map(Number);
    if (p.length === 4 && p.every((n) => n >= 0 && n <= 255)) return isBlockedIpv4Octets(p);
  }
  const hexMapped = v6.match(/^[:0]*ffff:([0-9a-f]{1,4}):([0-9a-f]{1,4})$/i);
  if (hexMapped) {
    const hi = parseInt(hexMapped[1], 16);
    const lo = parseInt(hexMapped[2], 16);
    return isBlockedIpv4Octets([(hi >> 8) & 255, hi & 255, (lo >> 8) & 255, lo & 255]);
  }
  if (v6.startsWith(':')) return false;
  const first = v6.split(':')[0] || '';
  const n = parseInt(first, 16);
  if (!Number.isNaN(n) && first.length) {
    if ((n & 0xfe00) === 0xfc00) return true;
    if ((n & 0xffc0) === 0xfe80) return true;
    if ((n & 0xff00) === 0xff00) return true;
  }
  return false;
}

/** Literal hosts matching Rust `is_blocked_download_host` (no DNS). Keep in sync with extension/media-utils.js. */
export function isBlockedLiteralHost(host: string): boolean {
  const h = host
    .trim()
    .toLowerCase()
    .replace(/^\[|\]$/g, '');
  if (!h) return true;
  if (h === 'localhost' || h.endsWith('.localhost')) return true;
  if (h.includes(':')) return isBlockedIpv6Literal(h);
  const m = h.match(/^(\d{1,3})\.(\d{1,3})\.(\d{1,3})\.(\d{1,3})$/);
  if (!m) return false;
  const o = [Number(m[1]), Number(m[2]), Number(m[3]), Number(m[4])];
  if (o.some((n) => n > 255)) return false;
  return isBlockedIpv4Octets(o);
}

/** Public http(s) URL only — no loopback/RFC1918/CGNAT literals, no other schemes. */
export function isPublicHttpUrl(value: string): boolean {
  const raw = value.trim();
  if (!raw || raw.length > 8192) return false;
  try {
    const parsed = new URL(raw);
    if (parsed.protocol !== 'http:' && parsed.protocol !== 'https:') return false;
    if (!parsed.hostname) return false;
    return !isBlockedLiteralHost(parsed.hostname);
  } catch {
    return false;
  }
}

/** text/uri-list or pasted lines — skip `#` comments. */
export function publicHttpUrlsFromText(text: string): string[] {
  const seen = new Set<string>();
  const out: string[] = [];
  for (const line of text.split(/\r?\n/)) {
    const value = line.trim();
    if (!value || value.startsWith('#') || !isPublicHttpUrl(value) || seen.has(value)) continue;
    seen.add(value);
    out.push(value);
  }
  return out;
}

/** CSV / space / uri-list paste. Do not split query commas (`?ids=1,2`). */
export function publicHttpUrlsFromMixedText(text: string): string[] {
  const lines: string[] = [];
  for (const line of text.split(/\r?\n/)) {
    const value = line.trim();
    if (!value || value.startsWith('#')) continue;
    lines.push(...value.split(/[,;\s]+(?=https?:\/\/)/i));
  }
  return publicHttpUrlsFromText(lines.join('\n'));
}

/** Cookie header only attaches when cookie_url origin matches the download (Rust). */
export function enqueueCookieUrl(
  cookies: string,
  referrer: string,
  urls: string[],
): string | undefined {
  if (!cookies.trim() || urls.length === 0) return undefined;
  const ref = referrer.trim();
  if (ref && isPublicHttpUrl(ref)) return ref;
  return urls[0];
}

/** GitHub html_url from the updater — never openUrl an unexpected host/path. */
export function isFalconGithubReleaseUrl(raw: string): boolean {
  try {
    const parsed = new URL(raw.trim());
    if (parsed.protocol !== 'https:') return false;
    if (parsed.username || parsed.password) return false;
    const host = parsed.hostname.toLowerCase();
    if (host !== 'github.com' && host !== 'www.github.com') return false;
    const path = parsed.pathname.toLowerCase();
    return path === '/batu3384/falcon-dm' || path.startsWith('/batu3384/falcon-dm/');
  } catch {
    return false;
  }
}
