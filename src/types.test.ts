import { describe, it, expect } from 'vitest';
import {
  formatBytes,
  formatSpeed,
  formatDuration,
  calculateETA,
  progressPercent,
  progressIndeterminate,
  loadedCountLabel,
  fileExtension,
  fileFullPath,
  displayPath,
  formatLocaleDate,
  formatShortDate,
} from './types';

describe('formatBytes', () => {
  it('returns 0 B for zero, negative and non-finite input', () => {
    expect(formatBytes(0)).toBe('0 B');
    expect(formatBytes(-5)).toBe('0 B');
    expect(formatBytes(Number.NaN)).toBe('0 B');
  });
  it('uses binary units with one decimal below 100', () => {
    expect(formatBytes(512)).toBe('512 B');
    expect(formatBytes(1536)).toBe('1.5 KB');
    expect(formatBytes(5 * 1024 * 1024)).toBe('5 MB');
    expect(formatBytes(150 * 1024 * 1024)).toBe('150 MB');
    expect(formatBytes(1024 ** 5 * 3)).toBe('3,072 TB');
  });
  it('uses the Turkish decimal comma', () => {
    expect(formatBytes(1536, 'tr')).toBe('1,5 KB');
  });
});

describe('formatSpeed / formatDuration', () => {
  it('localizes the per-second unit', () => {
    expect(formatSpeed(2048, 'en')).toBe('2 KB/s');
    expect(formatSpeed(2048, 'tr')).toBe('2 KB/sn');
  });
  it('formats remaining time', () => {
    expect(formatDuration(0)).toBe('');
    expect(formatDuration(42)).toBe('42s');
    expect(formatDuration(125)).toBe('2m 5s');
    expect(formatDuration(3700)).toBe('1h 1m');
    expect(formatDuration(3700, 'tr')).toBe('1 sa 1 dk');
    expect(formatDuration(86400 * 30)).toBe('over a week');
  });
  it('calculateETA needs speed and remaining bytes', () => {
    expect(calculateETA(1000, 0)).toBe('');
    expect(calculateETA(0, 100)).toBe('');
    expect(calculateETA(1000, 100)).toBe('10s');
  });
});

describe('progress helpers', () => {
  it('floors the percent so 100% means done', () => {
    expect(progressPercent({ total_size: 0, downloaded_size: 50 })).toBe(0);
    expect(progressPercent({ total_size: 200, downloaded_size: 50 })).toBe(25);
    expect(progressPercent({ total_size: 1000, downloaded_size: 999 })).toBe(99);
    expect(progressPercent({ total_size: 100, downloaded_size: 150 })).toBe(100);
  });
  it('is indeterminate while merging or when the size is unknown', () => {
    expect(
      progressIndeterminate({ total_size: 0, downloaded_size: 50, status: 'Downloading' }),
    ).toBe(true);
    expect(
      progressIndeterminate({ total_size: 100, downloaded_size: 100, status: 'Merging' }),
    ).toBe(true);
    expect(
      progressIndeterminate({ total_size: 100, downloaded_size: 50, status: 'Downloading' }),
    ).toBe(false);
    expect(progressIndeterminate({ total_size: 0, downloaded_size: 50, status: 'Paused' })).toBe(
      false,
    );
  });
  it('marks a truncated page', () => {
    expect(loadedCountLabel(200, true)).toBe('200+');
    expect(loadedCountLabel(3, false)).toBe('3');
  });
});

describe('file helpers', () => {
  it('extracts the extension', () => {
    expect(fileExtension('video.mp4')).toBe('MP4');
    expect(fileExtension('README')).toBe('');
    expect(fileExtension('.hidden')).toBe('');
    expect(fileExtension('trailing.')).toBe('');
  });
  it('joins paths and shortens the home folder', () => {
    expect(fileFullPath({ save_path: '/a/b/', filename: 'c.mp4' })).toBe('/a/b/c.mp4');
    expect(displayPath('/Users/demo/Downloads/x')).toBe('~/Downloads/x');
    expect(displayPath('/Volumes/Disk/x')).toBe('/Volumes/Disk/x');
  });
});

describe('dates', () => {
  it('returns empty for missing or invalid values', () => {
    expect(formatLocaleDate('')).toBe('');
    expect(formatLocaleDate('not-a-date')).toBe('');
    expect(formatLocaleDate(undefined)).toBe('');
    expect(formatShortDate(null)).toBe('');
  });
  it('shows a time for today and a date otherwise', () => {
    const now = new Date('2026-08-01T15:00:00');
    expect(formatShortDate('2026-08-01T09:05:00', 'en', now)).toMatch(/09:05|9:05/);
    expect(formatShortDate('2026-03-02T09:05:00', 'en', now)).toMatch(/Mar/);
    expect(formatShortDate('2024-03-02T09:05:00', 'en', now)).toMatch(/2024/);
    expect(formatLocaleDate('2026-08-01T00:00:00Z')).not.toBe('');
  });
});
