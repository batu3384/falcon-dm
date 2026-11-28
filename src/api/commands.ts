import { invoke } from '@tauri-apps/api/core';
import {
  DownloadArraySchema,
  DownloadSchema,
  DownloadStatsSchema,
  ExtensionStatusSchema,
  LogArraySchema,
  ScheduleSchema,
  SettingsSchema,
  ToolStatusSchema,
} from '../lib/schema';
import type {
  DownloadModel,
  DownloadStats,
  ExtensionStatus,
  LogEntry,
  ScheduleModel,
  SettingsModel,
  ToolStatus,
} from '../types';

// ponytail: centralized Tauri command layer. Every invoke() in the app used to
// repeat the raw command-name string with no type link to the backend; renaming
// a command silently broke the UI. Here each command is a typed function, and
// payloads returning from the backend are zod-validated before reaching state.

/** Convert a Tauri error (string | object) into a readable message. */
export function extractTauriError(e: unknown): string {
  let message = String(e);
  if (typeof e === 'string') message = e;
  else if (e && typeof e === 'object') {
    const err = e as Record<string, unknown>;
    if (typeof err.message === 'string') message = err.message;
  }
  if (/invoke/i.test(message) && /undefined/i.test(message)) {
    const lang = typeof document !== 'undefined' ? document.documentElement.lang || '' : '';
    return lang.startsWith('tr')
      ? 'Falcon DM motoru bağlı değil'
      : 'Falcon DM engine is not connected';
  }
  return message;
}

type Translate = (key: string, options?: Record<string, unknown>) => string;

const ERROR_CODES: Record<string, string> = {
  ERR_UNSUPPORTED_MAGNET: 'errors.unsupported_magnet',
  ERR_DISK_FULL: 'errors.disk_full',
  ERR_FFMPEG_MISSING: 'errors.ffmpeg_missing',
  ERR_YTDLP_MISSING: 'errors.ytdlp_missing',
  ERR_HLS_LIVE: 'errors.hls_live',
  ERR_DASH_LIVE: 'errors.dash_live',
  ERR_DASH_DRM: 'errors.dash_drm',
  ERR_REMOTE_CHANGED: 'errors.remote_changed',
  ERR_FILE_MISSING: 'errors.file_missing',
  ERR_PARTIAL_RESPONSE: 'errors.partial_response',
  ERR_CONTENT_ENCODING: 'errors.content_encoding',
  ERR_CHECKSUM_MISMATCH: 'errors.checksum_mismatch',
  ERR_CHECKSUM_INVALID: 'errors.checksum_invalid',
  ERR_VIDEO_UNAVAILABLE: 'errors.video_unavailable',
  ERR_VIDEO_PRIVATE: 'errors.video_private',
  ERR_LOGIN_REQUIRED: 'errors.login_required',
  'invalid url': 'errors.invalid_url',
  'not a real media url': 'errors.junk_media_url',
  'Server returned an error page instead of a file': 'errors.error_page',
  'Downloaded file is empty': 'errors.empty_file',
  'internal error: worker crashed': 'errors.internal',
};

/** Backend error codes and messages → text a user can act on. */
export function localizeDownloadError(message: string, t: Translate): string {
  const trimmed = message.trim();
  const key = ERROR_CODES[trimmed];
  if (key) return t(key);
  if (trimmed.startsWith('ERR_YTDLP_PATH_INVALID')) return t('errors.ytdlp_path_invalid');
  const http = /^HTTP (\d{3})$/.exec(trimmed);
  if (http) {
    const code = Number(http[1]);
    if (code === 401 || code === 403) return t('errors.http_forbidden', { code });
    if (code === 404 || code === 410) return t('errors.http_not_found', { code });
    if (code === 429) return t('errors.http_rate_limited', { code });
    if (code >= 500) return t('errors.http_server', { code });
    return t('errors.http_other', { code });
  }
  const lower = trimmed.toLowerCase();
  if (lower.startsWith('unsupported url scheme: magnet')) return t('errors.unsupported_magnet');
  if (lower.includes('private/loopback')) return t('errors.private_host');
  if (lower.startsWith('yt-dlp: ')) return trimmed.slice('yt-dlp: '.length);
  return trimmed;
}

export interface DownloadFilter {
  status?: string;
  statuses?: string[];
  category?: string;
  search?: string;
  limit?: number;
  offset?: number;
  before_id?: number;
  archived?: boolean;
}

export async function getDownloads(filter?: DownloadFilter): Promise<DownloadModel[]> {
  const raw = await invoke<unknown>('get_downloads', {
    filter: { limit: 200, offset: 0, ...filter },
  });
  return DownloadArraySchema.parse(raw);
}

export async function getDownload(id: number): Promise<DownloadModel> {
  return DownloadSchema.parse(await invoke<unknown>('get_download', { id }));
}

export interface AddDownloadParams {
  url: string;
  /** Omit to let the engine pick the server's or the page's name. */
  filename?: string;
  savePath?: string;
  referrer?: string;
  userAgent?: string;
  cookies?: string;
  cookieUrl?: string;
  /** yt-dlp format selector (YouTube quality). */
  format?: string;
  /** Lenient checksum input; canonicalized before send, verified on completion. */
  expectedHash?: string;
}

export async function addDownload(params: AddDownloadParams): Promise<DownloadModel> {
  // Tauri maps camelCase keys to the Rust command's snake_case parameters.
  return DownloadSchema.parse(await invoke<unknown>('add_download', { ...params }));
}

export async function pauseDownload(id: number): Promise<void> {
  await invoke('pause_download', { id });
}

export async function resumeDownload(id: number): Promise<void> {
  await invoke('resume_download', { id });
}

export async function pauseAllDownloads(): Promise<number> {
  return invoke<number>('pause_all_downloads');
}

export async function resumeAllDownloads(): Promise<number> {
  return invoke<number>('resume_all_downloads');
}

export async function removeDownload(id: number, deleteFile = false): Promise<void> {
  await invoke('remove_download', { id, deleteFile });
}

export async function changePriority(id: number, increase: boolean): Promise<void> {
  await invoke('change_priority', { id, increase });
}

/** Set one download's KB/s cap; 0 means it follows the global limit. */
export async function setDownloadSpeedLimit(id: number, speedLimitKbps: number): Promise<void> {
  await invoke('set_download_speed_limit', { id, speedLimitKbps });
}

export async function openFolder(path: string): Promise<void> {
  await invoke('open_folder', { path });
}

export async function openFile(path: string): Promise<void> {
  await invoke('open_file', { path });
}

export async function getSettings(): Promise<SettingsModel> {
  return SettingsSchema.parse(await invoke<unknown>('get_settings'));
}

export async function saveSettings(settings: SettingsModel): Promise<void> {
  await invoke('save_settings', { settings });
}

export async function getSchedule(): Promise<ScheduleModel> {
  return ScheduleSchema.parse(await invoke<unknown>('get_schedule'));
}

export async function setSchedule(params: {
  startTime: string | null;
  stopTime: string | null;
  active: boolean;
}): Promise<void> {
  await invoke('set_schedule', params);
}

export async function getExtensionStatus(): Promise<ExtensionStatus> {
  return ExtensionStatusSchema.parse(await invoke<unknown>('get_extension_status'));
}

export async function approveExtensionPair(extensionId: string): Promise<void> {
  await invoke('approve_extension_pair', { extensionId });
}

export async function rejectExtensionPair(extensionId: string): Promise<void> {
  await invoke('reject_extension_pair', { extensionId });
}

export async function revokeExtension(extensionId: string): Promise<void> {
  await invoke('revoke_extension', { extensionId });
}

export async function getToolStatus(): Promise<ToolStatus> {
  return ToolStatusSchema.parse(await invoke<unknown>('get_tool_status'));
}

export async function getLogs(level?: string): Promise<LogEntry[]> {
  return LogArraySchema.parse(await invoke<unknown>('get_logs', { level: level ?? null }));
}

export async function clearLogs(): Promise<void> {
  await invoke('clear_logs');
}

export async function getStats(): Promise<DownloadStats> {
  return DownloadStatsSchema.parse(await invoke<unknown>('get_stats'));
}

/** Rename and/or move a completed download's file. */
export async function moveDownload(
  id: number,
  newFilename?: string,
  newSavePath?: string,
): Promise<void> {
  await invoke('move_download', {
    id,
    newFilename: newFilename ?? null,
    newSavePath: newSavePath ?? null,
  });
}

/** Archived downloads leave the main list but stay in the DB and on disk. */
export async function archiveDownload(id: number, archived: boolean): Promise<void> {
  await invoke('archive_download', { id, archived });
}

/**
 * Replace the link of a failed or paused download with a fresh one.
 * The partial file is kept: the download resumes where it stopped.
 */
export async function refreshDownloadUrl(id: number, newUrl: string): Promise<void> {
  await invoke('refresh_download_url', { id, newUrl });
}

export interface UpdateCheckResult {
  current_version: string;
  latest_version: string | null;
  update_available: boolean;
  release_url: string | null;
}

export async function checkForUpdates(): Promise<UpdateCheckResult> {
  return invoke<UpdateCheckResult>('check_for_updates');
}

export async function readClipboard(): Promise<string> {
  return invoke<string>('read_clipboard');
}

export async function setUiLanguage(lang: string): Promise<void> {
  await invoke('set_ui_language', { lang });
}

/** Force the native window appearance for a manual theme ('system' clears it). */
export async function setWindowAppearance(theme: string): Promise<void> {
  await invoke('set_window_appearance', { theme });
}
