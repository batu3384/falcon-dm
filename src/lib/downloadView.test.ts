import { describe, expect, it } from 'vitest';
import {
  downloadMatchesSearch,
  downloadMatchesView,
  viewCounts,
  viewToDownloadFilter,
} from './downloadView';
import { makeRow } from '../test/fixtures';

describe('viewToDownloadFilter', () => {
  it('filters on the server so a view never shows a slice of another page', () => {
    expect(viewToDownloadFilter('all')).toEqual({ archived: false });
    expect(viewToDownloadFilter('active')).toEqual({
      archived: false,
      statuses: ['Downloading', 'Merging', 'Queued'],
    });
    expect(viewToDownloadFilter('failed')).toEqual({ archived: false, status: 'Failed' });
    expect(viewToDownloadFilter('document')).toEqual({ archived: false, category: 'Document' });
    expect(viewToDownloadFilter('archived')).toEqual({ archived: true });
  });
});

describe('downloadMatchesView', () => {
  it('matches the server filter for live updates', () => {
    const queued = makeRow({ status: 'Queued', category: 'Video' });
    expect(downloadMatchesView(queued, 'active')).toBe(true);
    expect(downloadMatchesView(queued, 'paused')).toBe(false);
    expect(downloadMatchesView(queued, 'video')).toBe(true);
    expect(downloadMatchesView(queued, 'music')).toBe(false);
  });

  it('keeps archived rows out of every view but Archive', () => {
    const archived = makeRow({ status: 'Completed', archived: true });
    expect(downloadMatchesView(archived, 'all')).toBe(false);
    expect(downloadMatchesView(archived, 'completed')).toBe(false);
    expect(downloadMatchesView(archived, 'archived')).toBe(true);
    expect(downloadMatchesView(makeRow(), 'archived')).toBe(false);
  });
});

describe('downloadMatchesSearch', () => {
  it('searches filename and URL, case-insensitively', () => {
    const row = makeRow({ filename: 'Report.PDF', url: 'https://docs.example.com/x' });
    expect(downloadMatchesSearch(row, '')).toBe(true);
    expect(downloadMatchesSearch(row, 'report')).toBe(true);
    expect(downloadMatchesSearch(row, 'DOCS.example')).toBe(true);
    expect(downloadMatchesSearch(row, 'video')).toBe(false);
  });
});

describe('viewCounts', () => {
  it('maps DB aggregates onto sidebar views', () => {
    const counts = viewCounts({
      active: 2,
      queued: 3,
      paused: 1,
      completed: 4,
      failed: 5,
      total_downloaded_bytes: 0,
      current_speed: 0,
      all: 15,
      archived: 6,
      video: 7,
      music: 8,
      document: 9,
      archive: 10,
      program: 11,
      free_disk_bytes: null,
      speed_limit_kbps: 0,
    });
    expect(counts).toMatchObject({
      all: 15,
      active: 5,
      paused: 1,
      failed: 5,
      archived: 6,
      program: 11,
    });
  });
});
