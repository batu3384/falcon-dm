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
};

describe('DownloadSchema', () => {
  it('accepts a well-formed download', () => {
    const parsed = DownloadSchema.parse(validDownload);
    expect(parsed.id).toBe(1);
    expect(parsed.status).toBe('Downloading');
    expect(parsed.archived).toBe(false);
    expect(parsed.attempts).toBe(0);
    expect(parsed.retry_at).toBeNull();
    expect(parsed.speed_limit_kbps).toBeNull();
  });

  it('keeps a per-download speed limit from the backend', () => {
    const parsed = DownloadSchema.parse({ ...validDownload, speed_limit_kbps: 512 });
    expect(parsed.speed_limit_kbps).toBe(512);
  });

  it('keeps retry bookkeeping from the backend', () => {
    const parsed = DownloadSchema.parse({ ...validDownload, attempts: 2, retry_at: 1_700_000 });
    expect(parsed.attempts).toBe(2);
    expect(parsed.retry_at).toBe(1_700_000);
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
  it('rejects more than 500 rows', () => {
    expect(() =>
      DownloadArraySchema.parse(Array.from({ length: 501 }, () => validDownload)),
    ).toThrow();
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
  it('only carries live statuses (stops arrive as full rows)', () => {
    expect(() =>
      ProgressPayloadSchema.parse({
        id: 1,
        downloaded_size: 100,
        total_size: 200,
        speed: 0,
        status: 'Failed',
        connections: 0,
      }),
    ).toThrow();
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

  it('keeps weekly schedule days and completion action fields', () => {
    const parsed = SettingsSchema.parse({
      theme: 'dark',
      default_download_path: '~/Downloads',
      max_concurrent_downloads: 3,
      max_connections_per_server: 16,
      proxy: null,
      schedule_days: [true, false, true, true, true, true, true],
      completion_action: 'script',
      completion_script: '/Users/demo/bin/done.sh',
    });
    expect(parsed.schedule_days).toEqual([true, false, true, true, true, true, true]);
    expect(parsed.completion_action).toBe('script');
    expect(parsed.completion_script).toBe('/Users/demo/bin/done.sh');
  });

  it('rejects malformed weekday masks and completion actions', () => {
    const base = {
      theme: 'dark',
      default_download_path: '~/Downloads',
      max_concurrent_downloads: 3,
      max_connections_per_server: 16,
      proxy: null,
    };
    expect(() => SettingsSchema.parse({ ...base, schedule_days: [true, false, true] })).toThrow();
    expect(() => SettingsSchema.parse({ ...base, completion_action: 'shutdown' })).toThrow();
  });
});

describe('ExtensionStatusSchema', () => {
  it('accepts approved ids and pending code requests', () => {
    const parsed = ExtensionStatusSchema.parse({
      approved_extension_ids: ['abc'],
      pending: [{ extension_id: 'abcdefghijklmnopabcdefghijklmnop', code: '482913' }],
    });
    expect(parsed.pending[0].code).toBe('482913');
  });
  it('rejects a malformed pairing code', () => {
    expect(() =>
      ExtensionStatusSchema.parse({
        approved_extension_ids: [],
        pending: [{ extension_id: 'abcdefghijklmnopabcdefghijklmnop', code: '12ab56' }],
      }),
    ).toThrow();
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
      all: 10,
      archived: 1,
      video: 2,
      music: 0,
      document: 0,
      archive: 0,
      program: 0,
    };
    expect(DownloadStatsSchema.parse(stats).active).toBe(1);
    expect(DownloadStatsSchema.parse(stats).all).toBe(10);
    expect(DownloadStatsSchema.parse(stats).free_disk_bytes).toBeNull();
    expect(
      DownloadStatsSchema.parse({ ...stats, free_disk_bytes: 5, speed_limit_kbps: 512 }),
    ).toMatchObject({ free_disk_bytes: 5, speed_limit_kbps: 512 });
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
      all: 10,
      archived: 1,
      video: 2,
      music: 0,
      document: 0,
      archive: 0,
      program: 0,
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
