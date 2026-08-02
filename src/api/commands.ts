import { invoke } from '@tauri-apps/api/core';
import {
  DownloadArraySchema,
  DownloadSchema,
  ScheduleSchema,
  SettingsSchema,
  ExtensionStatusSchema,
} from '../lib/schema';
import type { DownloadModel, SettingsModel, ScheduleModel } from '../types';

// ponytail: centralized Tauri command layer. Every invoke() in the app used to
// repeat the raw command-name string with no type link to the backend; renaming
// a command silently broke the UI. Here each command is a typed function, and
// payloads returning from the backend are zod-validated before reaching state.

/** Convert a Tauri error (string | object) into a readable message. */
export function extractTauriError(e: unknown): string {
  if (typeof e === 'string') return e;
  if (e && typeof e === 'object') {
    const err = e as Record<string, unknown>;
    if (typeof err.message === 'string') return err.message;
  }
  return String(e);
}

export interface DownloadFilter {
  status?: string;
  category?: string;
  search?: string;
  limit?: number;
  offset?: number;
  archived?: boolean;
}

export async function getDownloads(filter?: DownloadFilter): Promise<DownloadModel[]> {
  const raw = await invoke<unknown>('get_downloads', {
    filter: { limit: 500, ...filter },
  });
  return DownloadArraySchema.parse(raw) as DownloadModel[];
}

export async function getDownload(id: number): Promise<DownloadModel> {
  // get_download_status returns aria2 status; for a DB row we reuse get_downloads filter
  // is not available — backend exposes no single-row getter, so callers fetch the list.
  throw new Error(`getDownload(${id}): use getDownloads instead`);
}

export async function addDownload(params: {
  url: string;
  filename: string;
  savePath: string;
  referrer?: string;
  userAgent?: string;
  cookies?: string;
}): Promise<DownloadModel> {
  // ponytail: Tauri's invoke auto-converts camelCase keys to the Rust command's
  // snake_case params, so callers pass idiomatic TS names.
  const raw = await invoke<unknown>('add_download', params);
  return DownloadSchema.parse(raw) as DownloadModel;
}

export async function pauseDownload(id: number): Promise<void> {
  await invoke('pause_download', { id });
}

export async function resumeDownload(id: number): Promise<void> {
  await invoke('resume_download', { id });
}

export async function removeDownload(id: number, deleteFile = false): Promise<void> {
  await invoke('remove_download', { id, deleteFile });
}

export async function changePriority(id: number, increase: boolean): Promise<void> {
  await invoke('change_priority', { id, increase });
}

export async function openFolder(path: string): Promise<void> {
  await invoke('open_folder', { path });
}

export async function openFile(path: string): Promise<void> {
  await invoke('open_file', { path });
}

export async function getSettings(): Promise<SettingsModel> {
  const raw = await invoke<unknown>('get_settings');
  return SettingsSchema.parse(raw) as SettingsModel;
}

export async function saveSettings(settings: SettingsModel): Promise<void> {
  await invoke('save_settings', { settings });
}

export async function getSchedule(): Promise<ScheduleModel> {
  const raw = await invoke<unknown>('get_schedule');
  return ScheduleSchema.parse(raw) as ScheduleModel;
}

export async function setSchedule(params: {
  startTime: string | null;
  stopTime: string | null;
  active: boolean;
}): Promise<void> {
  await invoke('set_schedule', params);
}

export interface ExtensionStatus {
  has_token: boolean;
  approved_extension_ids: string[];
  pending_pair_id: string | null;
}

export async function getExtensionStatus(): Promise<ExtensionStatus> {
  const raw = await invoke<unknown>('get_extension_status');
  return ExtensionStatusSchema.parse(raw) as ExtensionStatus;
}

export async function approveExtensionPair(extensionId: string): Promise<void> {
  await invoke('approve_extension_pair', { extensionId });
}

export async function resetExtensionPin(): Promise<void> {
  await invoke('reset_extension_pin');
}

export async function getPendingPair(): Promise<string | null> {
  return invoke<string | null>('get_pending_pair');
}

// ponytail: log panel — pulls the in-memory ring buffer the fan-out logger
// captures. Optional level filter narrows server-side (e.g. "ERROR").
export interface LogEntry {
  ts: number;
  level: string;
  target: string;
  message: string;
}

export async function getLogs(level?: string): Promise<LogEntry[]> {
  return invoke<LogEntry[]>('get_logs', { level: level ?? null });
}

export async function clearLogs(): Promise<void> {
  await invoke('clear_logs');
}

// ponytail: aggregate statistics for the Stats panel.
export interface DownloadStats {
  active: number;
  queued: number;
  completed: number;
  failed: number;
  total_downloaded_bytes: number;
  current_speed: number;
}

export async function getStats(): Promise<DownloadStats> {
  return invoke<DownloadStats>('get_stats');
}

// ponytail: rename and/or move a completed download's file.
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

// ponytail: toggle the archived flag. Archived downloads are hidden from the
// active list but kept in the DB + on disk. The "Archived" sidebar filter shows
// them via get_downloads({ archived: true }).
export async function archiveDownload(id: number, archived: boolean): Promise<void> {
  await invoke('archive_download', { id, archived });
}

/// Build a cURL command string from a download's URL + headers. For the "Copy as
/// cURL" context-menu action — lets users replicate the request in a terminal.
export function buildCurlCommand(dl: {
  url: string;
  user_agent?: string | null;
  referrer?: string | null;
  cookies?: string | null;
}): string {
  const parts = ['curl', '-L', shquote(dl.url)];
  if (dl.user_agent) parts.push('-H', shquote(`User-Agent: ${dl.user_agent}`));
  if (dl.referrer) parts.push('-H', shquote(`Referer: ${dl.referrer}`));
  if (dl.cookies) parts.push('-H', shquote(`Cookie: ${dl.cookies}`));
  parts.push('-o', shquote(dl.url.split('/').pop()?.split('?')[0] || 'download.bin'));
  return parts.join(' ');
}

function shquote(s: string): string {
  // Single-quote wrap, escaping embedded single quotes.
  return `'${s.replace(/'/g, "'\\''")}'`;
}
