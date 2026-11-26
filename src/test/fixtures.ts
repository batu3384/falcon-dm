import type { DownloadModel } from '../types';

let nextId = 100;

/** A valid download row; override what the test cares about. */
export function makeRow(overrides: Partial<DownloadModel> = {}): DownloadModel {
  nextId += 1;
  return {
    id: nextId,
    url: `https://cdn.example.com/file-${nextId}.bin`,
    filename: `file-${nextId}.bin`,
    save_path: '/Users/demo/Downloads',
    total_size: 1000,
    downloaded_size: 0,
    status: 'Queued',
    category: 'Other',
    speed: 0,
    segments: 1,
    priority: 1,
    created_at: '2026-08-01T00:00:00Z',
    completed_at: null,
    error_message: null,
    referrer: null,
    user_agent: null,
    archived: false,
    attempts: 0,
    retry_at: null,
    ...overrides,
  };
}
