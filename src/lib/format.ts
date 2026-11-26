// Number and time formatting in the UI language ("2,1 GB" in Turkish).

function localeOf(lang?: string): string {
  return lang?.startsWith('tr') ? 'tr-TR' : 'en-US';
}

const UNITS = ['B', 'KB', 'MB', 'GB', 'TB'];

export function formatBytes(bytes: number, lang?: string, decimals = 1): string {
  if (!Number.isFinite(bytes) || bytes <= 0) return '0 B';
  const i = Math.min(Math.floor(Math.log(bytes) / Math.log(1024)), UNITS.length - 1);
  const value = bytes / 1024 ** i;
  const digits = i === 0 ? 0 : value >= 100 ? 0 : decimals;
  const text = new Intl.NumberFormat(localeOf(lang), {
    maximumFractionDigits: digits,
    minimumFractionDigits: 0,
  }).format(value);
  return `${text} ${UNITS[i]}`;
}

export function formatSpeed(bytesPerSecond: number, lang?: string): string {
  return `${formatBytes(bytesPerSecond, lang)}/${lang?.startsWith('tr') ? 'sn' : 's'}`;
}

/** Remaining time, or '' when it cannot be estimated. */
export function formatDuration(seconds: number, lang?: string): string {
  if (!Number.isFinite(seconds) || seconds <= 0) return '';
  const tr = lang?.startsWith('tr');
  const s = Math.ceil(seconds);
  if (s < 60) return tr ? `${s} sn` : `${s}s`;
  if (s < 3600) {
    const m = Math.floor(s / 60);
    const rest = s % 60;
    return tr ? `${m} dk ${rest} sn` : `${m}m ${rest}s`;
  }
  if (s < 86400 * 7) {
    const h = Math.floor(s / 3600);
    const m = Math.floor((s % 3600) / 60);
    return tr ? `${h} sa ${m} dk` : `${h}h ${m}m`;
  }
  return tr ? '1 haftadan fazla' : 'over a week';
}

export function calculateETA(remainingBytes: number, speed: number, lang?: string): string {
  if (speed <= 0 || remainingBytes <= 0) return '';
  return formatDuration(remainingBytes / speed, lang);
}

export function progressPercent(dl: { total_size: number; downloaded_size: number }): number {
  if (dl.total_size <= 0) return 0;
  return Math.min(100, Math.floor((dl.downloaded_size / dl.total_size) * 100));
}

/** True when bytes are moving but the total is unknown, or the file is being muxed. */
export function progressIndeterminate(dl: {
  total_size: number;
  downloaded_size: number;
  status: string;
}): boolean {
  if (dl.status === 'Merging') return true;
  return dl.status === 'Downloading' && dl.total_size <= 0;
}

/** Page size for lists that still have another SQL page. */
export function loadedCountLabel(count: number, hasMore: boolean): string {
  return hasMore ? `${count}+` : String(count);
}

export function fileExtension(filename: string): string {
  const dot = filename.lastIndexOf('.');
  return dot > 0 && dot < filename.length - 1 ? filename.slice(dot + 1).toUpperCase() : '';
}

export function fileFullPath(dl: { save_path: string; filename: string }): string {
  const base = dl.save_path.replace(/\/+$/, '');
  return `${base}/${dl.filename}`;
}

/** `/Users/me/Downloads/x` → `~/Downloads/x` for display. */
export function displayPath(path: string): string {
  return path.replace(/^\/Users\/[^/]+/, '~');
}

export function formatLocaleDate(raw: string | null | undefined, lang?: string): string {
  if (!raw || !raw.trim()) return '';
  const parsed = new Date(raw);
  if (Number.isNaN(parsed.getTime())) return '';
  return parsed.toLocaleString(localeOf(lang), { dateStyle: 'medium', timeStyle: 'short' });
}

/** "14:05" today, "21 Kas" this year, "21.11.2025" otherwise. */
export function formatShortDate(raw: string | null | undefined, lang?: string, now = new Date()) {
  if (!raw) return '';
  const date = new Date(raw);
  if (Number.isNaN(date.getTime())) return '';
  const locale = localeOf(lang);
  if (date.toDateString() === now.toDateString()) {
    return date.toLocaleTimeString(locale, { hour: '2-digit', minute: '2-digit' });
  }
  if (date.getFullYear() === now.getFullYear()) {
    return date.toLocaleDateString(locale, { day: 'numeric', month: 'short' });
  }
  return date.toLocaleDateString(locale);
}
