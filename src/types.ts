// ponytail: union types for backend string enums. Previously these were loose
// `string`, so a typo like "Complete" instead of "Completed" would silently
// break filtering/UI. Now the compiler (and zod, see lib/schema.ts) catch it.
export type DownloadStatus =
  'Queued' | 'Downloading' | 'Paused' | 'Completed' | 'Failed' | 'Merging';

export type DownloadCategory = 'Video' | 'Music' | 'Document' | 'Archive' | 'Program' | 'Other';

export interface DownloadModel {
  id: number;
  url: string;
  filename: string;
  save_path: string;
  total_size: number;
  downloaded_size: number;
  status: DownloadStatus;
  category: DownloadCategory;
  speed: number;
  segments: number;
  priority: number;
  created_at: string;
  completed_at: string | null;
  error_message: string | null;
  referrer: string | null;
  user_agent: string | null;
  aria2_gid: string | null;
  archived?: boolean;
}

export interface SettingsModel {
  theme: string;
  default_download_path: string;
  max_concurrent_downloads: number;
  max_connections_per_server: number;
  proxy: string | null;
  api_token?: string;
  speed_limit_kbps?: number;
  category_paths?: Record<string, string>;
  allowed_extension_ids?: string[];
  ytdlp_path?: string;
  schedule_active?: boolean;
  schedule_start?: string | null;
  schedule_stop?: string | null;
  download_profiles?: DownloadProfile[];
}

// ponytail: per-site download profile. Matched by url_pattern substring.
export interface DownloadProfile {
  name: string;
  url_pattern: string;
  user_agent?: string | null;
  referrer?: string | null;
  cookies?: string | null;
  save_subdir?: string | null;
}

export interface ProgressPayload {
  id: number;
  downloaded_size: number;
  total_size: number;
  speed: number;
  status: DownloadStatus;
  connections: number;
}

export interface ScheduleModel {
  start_time: string | null;
  stop_time: string | null;
  active: boolean;
}

export function formatBytes(bytes: number, decimals = 1): string {
  if (!bytes || bytes <= 0) return '0 B';
  const k = 1024;
  const dm = decimals < 0 ? 0 : decimals;
  const sizes = ['B', 'KB', 'MB', 'GB', 'TB'];
  const i = Math.min(Math.floor(Math.log(bytes) / Math.log(k)), sizes.length - 1);
  return `${parseFloat((bytes / Math.pow(k, i)).toFixed(dm))} ${sizes[i]}`;
}

export function calculateETA(remainingBytes: number, speed: number): string {
  if (speed <= 0 || remainingBytes <= 0) return '';
  const seconds = Math.ceil(remainingBytes / speed);
  if (seconds < 60) return `${seconds}s`;
  if (seconds < 3600) {
    const m = Math.floor(seconds / 60);
    const s = seconds % 60;
    return `${m}m ${s}s`;
  }
  const h = Math.floor(seconds / 3600);
  const m = Math.floor((seconds % 3600) / 60);
  return `${h}h ${m}m`;
}

export function progressPercent(dl: { total_size: number; downloaded_size: number }): number {
  if (dl.total_size <= 0) return 0;
  return Math.min(100, Math.round((dl.downloaded_size / dl.total_size) * 100));
}

export function fileExtension(filename: string): string {
  return filename.includes('.') ? filename.split('.').pop()?.toUpperCase() || '' : '';
}

export function fileFullPath(dl: { save_path: string; filename: string }): string {
  const base = dl.save_path.replace(/\/$/, '');
  return `${base}/${dl.filename}`;
}

export function applyTheme(theme: string) {
  // ponytail: a manual theme choice sets data-theme-manual="true" so the OS
  // theme-change listener no longer clobbers the user's explicit preference.
  // Previously applyTheme never set that flag, so toggling dark/light in
  // Settings was silently overridden the moment the OS theme changed.
  if (theme === 'dark' || theme === 'light') {
    document.documentElement.setAttribute('data-theme', theme);
    document.documentElement.setAttribute('data-theme-manual', 'true');
  } else {
    document.documentElement.removeAttribute('data-theme-manual');
    const dark = window.matchMedia('(prefers-color-scheme: dark)').matches;
    document.documentElement.setAttribute('data-theme', dark ? 'dark' : 'light');
  }
}

// ponytail: matchMedia listener kept simple — OS theme change while app is open should update instantly.
let _themeMq: MediaQueryList | null = null;
export function watchSystemTheme() {
  if (_themeMq) return;
  _themeMq = window.matchMedia('(prefers-color-scheme: dark)');
  const handler = () => {
    if (document.documentElement.getAttribute('data-theme-manual') !== 'true') {
      document.documentElement.setAttribute('data-theme', _themeMq!.matches ? 'dark' : 'light');
    }
  };
  _themeMq.addEventListener('change', handler);
}
