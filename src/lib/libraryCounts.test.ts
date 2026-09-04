import { describe, expect, it } from 'vitest';
import { libraryCountsFromStats } from './libraryCounts';
import type { DownloadStats } from '../api/commands';

const stats: DownloadStats = {
  active: 2,
  queued: 3,
  paused: 1,
  completed: 10,
  failed: 4,
  total_downloaded_bytes: 100,
  current_speed: 0,
  all: 20,
  archived: 5,
  video: 8,
  music: 1,
  document: 2,
  archive: 3,
  program: 0,
};

describe('libraryCountsFromStats', () => {
  it('maps library + category badges independently of the visible list', () => {
    const counts = libraryCountsFromStats(stats);
    expect(counts['All Downloads']).toBe(20);
    expect(counts.Downloading).toBe(2);
    expect(counts.Paused).toBe(4);
    expect(counts.Failed).toBe(4);
    expect(counts.Completed).toBe(10);
    expect(counts.Archived).toBe(5);
    expect(counts.Video).toBe(8);
    expect(counts.Compressed).toBe(3);
    expect(counts.Programs).toBe(0);
    expect(counts.pausable).toBe(5);
    expect(counts.resumable).toBe(5);
  });
});
