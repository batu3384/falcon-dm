import { z } from 'zod';

// ponytail: zod runtime validation for Tauri IPC payloads. Model types are
// `z.infer` of these schemas (re-exported from types.ts). If the schema drifts
// (a field renamed, a status string changes), parse throws instead of letting
// `undefined` corrupt UI state.

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
  archived: z.boolean().optional().default(false),
  attempts: z.number().int().nonnegative().optional().default(0),
  retry_at: z.number().nullable().optional().default(null),
  /** Canonical `algo:hex` the finished file was verified against (sha256/md5). */
  expected_hash: z.string().nullable().optional().default(null),
  /** Per-download KB/s cap; null means the global limit. */
  speed_limit_kbps: z.number().int().nonnegative().nullable().optional().default(null),
});

export const DownloadArraySchema = z.array(DownloadSchema).max(500);

/** Byte counters from a running worker. Status changes arrive as full rows. */
export const ProgressPayloadSchema = z.object({
  id: z.number(),
  downloaded_size: z.number().nonnegative(),
  total_size: z.number().nonnegative(),
  speed: z.number().finite().nonnegative(),
  status: z.enum(['Downloading', 'Merging']),
  connections: z.number().int().nonnegative(),
});

export const DownloadProfileSchema = z.object({
  name: z.string(),
  // Host or origin (example.com / https://example.com:8443). Rust matches hostname, not substring.
  url_pattern: z.string(),
  user_agent: z.string().nullable().optional(),
  referrer: z.string().nullable().optional(),
  cookies: z.string().nullable().optional(),
  save_subdir: z.string().nullable().optional(),
});

const WeekdayMaskSchema = z.tuple([
  z.boolean(),
  z.boolean(),
  z.boolean(),
  z.boolean(),
  z.boolean(),
  z.boolean(),
  z.boolean(),
]);

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
  ytdlp_use_browser_cookies: z.boolean().optional(),
  schedule_active: z.boolean().optional(),
  schedule_start: z.string().nullable().optional(),
  schedule_stop: z.string().nullable().optional(),
  schedule_days: WeekdayMaskSchema.optional(),
  completion_action: z.enum(['none', 'sleep', 'script']).optional(),
  completion_script: z.string().optional(),
  download_profiles: z.array(DownloadProfileSchema).optional(),
});

export const ScheduleSchema = z.object({
  start_time: z.string().nullable(),
  stop_time: z.string().nullable(),
  active: z.boolean(),
});

// Chromium ids are 32 lowercase hex-ish chars [a-p]; Firefox uses a bare or
// braced UUID, or an email-style Gecko id — mirrors `is_valid_extension_id` in the backend.
const ExtensionIdSchema = z
  .string()
  .max(256)
  .regex(
    /^([a-p]{32}|\{?[0-9a-fA-F]{8}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{12}\}?|[\w.+-]+@[\w-]+(\.[\w-]+)+)$/,
  );

export const PairRequestSchema = z.object({
  extension_id: ExtensionIdSchema,
  code: z.string().regex(/^\d{6}$/),
});

export const ExtensionStatusSchema = z.object({
  approved_extension_ids: z.array(z.string()),
  pending: z.array(PairRequestSchema).max(16),
});

export const ToolStatusSchema = z.object({
  ytdlp_path: z.string().nullable(),
  ytdlp_version: z.string().nullable(),
  ytdlp_error: z.string().nullable(),
  ffmpeg_path: z.string().nullable(),
});

export const LogEntrySchema = z.object({
  ts: z.number().finite().nonnegative(),
  level: z.string(),
  target: z.string(),
  message: z.string(),
});

export const LogArraySchema = z.array(LogEntrySchema).max(1000);

export const DownloadStatsSchema = z.object({
  active: z.number().int().nonnegative(),
  queued: z.number().int().nonnegative(),
  paused: z.number().int().nonnegative(),
  completed: z.number().int().nonnegative(),
  failed: z.number().int().nonnegative(),
  total_downloaded_bytes: z.number().finite().nonnegative(),
  current_speed: z.number().finite().nonnegative(),
  all: z.number().int().nonnegative(),
  archived: z.number().int().nonnegative(),
  video: z.number().int().nonnegative(),
  music: z.number().int().nonnegative(),
  document: z.number().int().nonnegative(),
  archive: z.number().int().nonnegative(),
  program: z.number().int().nonnegative(),
  free_disk_bytes: z.number().finite().nonnegative().nullable().optional().default(null),
  speed_limit_kbps: z.number().int().nonnegative().optional().default(0),
});

export type DownloadStatus = z.infer<typeof DownloadStatusEnum>;
export type DownloadCategory = z.infer<typeof DownloadCategoryEnum>;
export type DownloadModel = z.infer<typeof DownloadSchema>;
export type ProgressPayload = z.infer<typeof ProgressPayloadSchema>;
export type DownloadProfile = z.infer<typeof DownloadProfileSchema>;
export type SettingsModel = z.infer<typeof SettingsSchema>;
export type ScheduleModel = z.infer<typeof ScheduleSchema>;
export type PairRequest = z.infer<typeof PairRequestSchema>;
export type ExtensionStatus = z.infer<typeof ExtensionStatusSchema>;
export type ToolStatus = z.infer<typeof ToolStatusSchema>;
export type LogEntry = z.infer<typeof LogEntrySchema>;
export type DownloadStats = z.infer<typeof DownloadStatsSchema>;
