import { z } from 'zod';

// ponytail: zod runtime validation for Tauri IPC payloads. The TS types in
// types.ts are a compile-time contract only — they trust that the backend
// actually sends well-formed data. If the schema drifts (a field renamed, a
// status string changes), the UI would silently break with `undefined` values.
// These schemas parse every invoke()/listen() result so malformed payloads
// throw a loud, catchable error instead of corrupting UI state.

export const DownloadStatusEnum = z.enum([
  'Queued',
  'Downloading',
  'Paused',
  'Completed',
  'Failed',
  'Merging',
]);

export const DownloadCategoryEnum = z.enum([
  'Video',
  'Music',
  'Document',
  'Archive',
  'Program',
  'Other',
]);

export const DownloadSchema = z.object({
  id: z.number(),
  url: z.string(),
  filename: z.string(),
  save_path: z.string(),
  total_size: z.number(),
  downloaded_size: z.number(),
  status: DownloadStatusEnum,
  category: DownloadCategoryEnum,
  speed: z.number(),
  segments: z.number(),
  priority: z.number(),
  created_at: z.string(),
  completed_at: z.string().nullable(),
  error_message: z.string().nullable(),
  referrer: z.string().nullable(),
  user_agent: z.string().nullable(),
  aria2_gid: z.string().nullable(),
  archived: z.boolean().optional().default(false),
});

export const DownloadArraySchema = z.array(DownloadSchema);

export const ProgressPayloadSchema = z.object({
  id: z.number(),
  downloaded_size: z.number(),
  total_size: z.number(),
  speed: z.number(),
  status: DownloadStatusEnum,
  connections: z.number(),
});

export const DownloadProfileSchema = z.object({
  name: z.string(),
  url_pattern: z.string(),
  user_agent: z.string().nullable().optional(),
  referrer: z.string().nullable().optional(),
  cookies: z.string().nullable().optional(),
  save_subdir: z.string().nullable().optional(),
});

export const SettingsSchema = z.object({
  theme: z.string(),
  default_download_path: z.string(),
  max_concurrent_downloads: z.number(),
  max_connections_per_server: z.number(),
  proxy: z.string().nullable(),
  api_token: z.string().optional(),
  speed_limit_kbps: z.number().optional(),
  category_paths: z.record(z.string(), z.string()).optional(),
  allowed_extension_ids: z.array(z.string()).optional(),
  ytdlp_path: z.string().optional(),
  schedule_active: z.boolean().optional(),
  schedule_start: z.string().nullable().optional(),
  schedule_stop: z.string().nullable().optional(),
  download_profiles: z.array(DownloadProfileSchema).optional(),
});

export const ScheduleSchema = z.object({
  start_time: z.string().nullable(),
  stop_time: z.string().nullable(),
  active: z.boolean(),
});

export const ExtensionStatusSchema = z.object({
  has_token: z.boolean(),
  approved_extension_ids: z.array(z.string()),
  pending_pair_id: z.string().nullable(),
});

export const LogEntrySchema = z.object({
  ts: z.number(),
  level: z.string(),
  target: z.string(),
  message: z.string(),
});

export const LogArraySchema = z.array(LogEntrySchema);

export const DownloadStatsSchema = z.object({
  active: z.number(),
  queued: z.number(),
  completed: z.number(),
  failed: z.number(),
  total_downloaded_bytes: z.number(),
  current_speed: z.number(),
});

export type ParsedDownload = z.infer<typeof DownloadSchema>;
export type ParsedProgress = z.infer<typeof ProgressPayloadSchema>;
export type ParsedSettings = z.infer<typeof SettingsSchema>;
