import { describe, it, expect } from 'vitest';
import {
  DownloadSchema,
  DownloadArraySchema,
  ProgressPayloadSchema,
  SettingsSchema,
  ExtensionStatusSchema,
  DownloadStatusEnum,
  DownloadCategoryEnum,
  LogArraySchema,
  DownloadStatsSchema,
} from './schema';

// ponytail: schema tests guard the boundary between Rust and TS. If the backend
// renames a field or changes a status string, these fail loudly instead of
// silently corrupting UI state.

const validDownload = {
  id: 1,
  url: 'https://example.com/file.mp4',
  filename: 'file.mp4',
  save_path: '/tmp',
  total_size: 1024,
  downloaded_size: 512,
  status: 'Downloading',
  category: 'Video',
  speed: 100.5,
  segments: 8,
  priority: 1,
  created_at: '2026-08-01T00:00:00Z',
  completed_at: null,
  error_message: null,
  referrer: null,
  user_agent: null,
  cookies: null,
  aria2_gid: null,
};

describe('DownloadSchema', () => {
  it('accepts a well-formed download', () => {
    const parsed = DownloadSchema.parse(validDownload);
    expect(parsed.id).toBe(1);
    expect(parsed.status).toBe('Downloading');
    expect(parsed.archived).toBe(false);
  });

  it('does not expose cookies in download payload schema', () => {
    const parsed = DownloadSchema.parse({
      ...validDownload,
      cookies: 'sid=secret',
    });
    expect(parsed).not.toHaveProperty('cookies');
  });

  it('rejects an unknown status string', () => {
    expect(() => DownloadSchema.parse({ ...validDownload, status: 'Complete' })).toThrow();
  });

  it('rejects an unknown category string', () => {
    expect(() => DownloadSchema.parse({ ...validDownload, category: 'Movies' })).toThrow();
  });

  it('rejects a missing required field', () => {
    const { id, ...missing } = validDownload;
    expect(id).toBeDefined();
    expect(() => DownloadSchema.parse(missing)).toThrow();
  });
});

describe('DownloadArraySchema', () => {
  it('parses a list', () => {
    const parsed = DownloadArraySchema.parse([validDownload, validDownload]);
    expect(parsed).toHaveLength(2);
  });
  it('rejects a non-array', () => {
    expect(() => DownloadArraySchema.parse(validDownload)).toThrow();
  });
});

describe('ProgressPayloadSchema', () => {
  it('accepts a valid payload', () => {
    const parsed = ProgressPayloadSchema.parse({
      id: 1,
      downloaded_size: 100,
      total_size: 200,
      speed: 50,
      status: 'Downloading',
      connections: 8,
    });
    expect(parsed.status).toBe('Downloading');
  });
  it('rejects an invalid status', () => {
    expect(() =>
      ProgressPayloadSchema.parse({
        id: 1,
        downloaded_size: 100,
        total_size: 200,
        speed: 50,
        status: 'Weird',
        connections: 8,
      }),
    ).toThrow();
  });
});

describe('SettingsSchema', () => {
  it('accepts a full settings object', () => {
    const parsed = SettingsSchema.parse({
      theme: 'dark',
      default_download_path: '~/Downloads',
      max_concurrent_downloads: 3,
      max_connections_per_server: 16,
      proxy: null,
    });
    expect(parsed.theme).toBe('dark');
  });
});

describe('ExtensionStatusSchema', () => {
  it('accepts approved + pending + token state', () => {
    const parsed = ExtensionStatusSchema.parse({
      has_token: true,
      approved_extension_ids: ['abc'],
      pending_pair_id: null,
    });
    expect(parsed.has_token).toBe(true);
  });
});

describe('diagnostic payload schemas', () => {
  it('parses log entries and rejects malformed entries', () => {
    expect(
      LogArraySchema.parse([{ ts: 1, level: 'ERROR', target: 'app', message: 'failed' }]),
    ).toHaveLength(1);
    expect(() => LogArraySchema.parse([{ ts: 'now' }])).toThrow();
  });

  it('parses download stats and rejects malformed counters', () => {
    const stats = {
      active: 1,
      queued: 2,
      paused: 0,
      completed: 3,
      failed: 4,
      total_downloaded_bytes: 5,
      current_speed: 6,
    };
    expect(DownloadStatsSchema.parse(stats).active).toBe(1);
    expect(() => DownloadStatsSchema.parse({ ...stats, active: '1' })).toThrow();
  });

  it('rejects negative stats and non-finite speed', () => {
    const stats = {
      active: 1,
      queued: 2,
      paused: 0,
      completed: 3,
      failed: 4,
      total_downloaded_bytes: 5,
      current_speed: 6,
    };
    expect(() => DownloadStatsSchema.parse({ ...stats, active: -1 })).toThrow();
    expect(() => DownloadStatsSchema.parse({ ...stats, current_speed: Number.NaN })).toThrow();
  });

  it('rejects malformed log entries', () => {
    expect(() => LogArraySchema.parse([{ level: 'ERROR' }])).toThrow();
  });
});

describe('enum exports', () => {
  it('DownloadStatusEnum covers all six statuses', () => {
    const all = DownloadStatusEnum.options;
    expect(all).toEqual(['Queued', 'Downloading', 'Paused', 'Completed', 'Failed', 'Merging']);
  });
  it('DownloadCategoryEnum covers all six categories', () => {
    const all = DownloadCategoryEnum.options;
    expect(all).toEqual(['Video', 'Music', 'Document', 'Archive', 'Program', 'Other']);
  });
});
