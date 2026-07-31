export interface DownloadModel {
  id: number;
  url: string;
  filename: string;
  save_path: string;
  total_size: number;
  downloaded_size: number;
  status: string;
  category: string;
  speed: number;
  segments: number;
  priority: number;
  created_at: string;
  completed_at: string | null;
  error_message: string | null;
  referrer: string | null;
  user_agent: string | null;
  cookies: string | null;
  aria2_gid: string | null;
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
}

export interface ProgressPayload {
  id: number;
  downloaded_size: number;
  total_size: number;
  speed: number;
  status: string;
  connections: number;
}

export interface ScheduleModel {
  start_time: string | null;
  stop_time: string | null;
  active: boolean;
}

export function formatBytes(bytes: number, decimals = 1): string {
  if (!bytes || bytes <= 0) return "0 B";
  const k = 1024;
  const dm = decimals < 0 ? 0 : decimals;
  const sizes = ["B", "KB", "MB", "GB", "TB"];
  const i = Math.min(Math.floor(Math.log(bytes) / Math.log(k)), sizes.length - 1);
  return `${parseFloat((bytes / Math.pow(k, i)).toFixed(dm))} ${sizes[i]}`;
}

export function calculateETA(remainingBytes: number, speed: number): string {
  if (speed <= 0 || remainingBytes <= 0) return "";
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
  return filename.includes(".")
    ? filename.split(".").pop()?.toUpperCase() || ""
    : "";
}

export function fileFullPath(dl: { save_path: string; filename: string }): string {
  const base = dl.save_path.replace(/\/$/, "");
  return `${base}/${dl.filename}`;
}

export function applyTheme(theme: string) {
  if (theme === "dark") {
    document.documentElement.setAttribute("data-theme", "dark");
  } else if (theme === "light") {
    document.documentElement.setAttribute("data-theme", "light");
  } else {
    const dark = window.matchMedia("(prefers-color-scheme: dark)").matches;
    document.documentElement.setAttribute("data-theme", dark ? "dark" : "light");
  }
}
