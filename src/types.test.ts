import { describe, it, expect } from 'vitest';
import { formatBytes, calculateETA, progressPercent, fileExtension, fileFullPath } from './types';

// ponytail: pure formatting helpers are the easiest high-value tests — they're
// used everywhere and a regression (e.g. NaN bytes, off-by-one percent) would
// be visible across the whole UI.

describe('formatBytes', () => {
  it('returns 0 B for zero/negative', () => {
    expect(formatBytes(0)).toBe('0 B');
    expect(formatBytes(-5)).toBe('0 B');
  });
  it('formats bytes', () => {
    expect(formatBytes(512)).toBe('512 B');
  });
  it('formats kilobytes', () => {
    expect(formatBytes(1024)).toBe('1 KB');
    expect(formatBytes(1536)).toBe('1.5 KB');
  });
  it('formats megabytes', () => {
    expect(formatBytes(1048576)).toBe('1 MB');
  });
  it('caps at terabytes', () => {
    expect(formatBytes(Math.pow(1024, 5))).toMatch(/TB/);
  });
  it('respects decimals param', () => {
    expect(formatBytes(1536, 2)).toBe('1.5 KB');
  });
});

describe('calculateETA', () => {
  it('returns empty for zero/negative speed', () => {
    expect(calculateETA(100, 0)).toBe('');
    expect(calculateETA(100, -1)).toBe('');
  });
  it('returns empty for zero/negative remaining', () => {
    expect(calculateETA(0, 100)).toBe('');
  });
  it('formats seconds', () => {
    expect(calculateETA(500, 100)).toBe('5s');
  });
  it('formats minutes', () => {
    expect(calculateETA(60000, 1000)).toBe('1m 0s');
  });
  it('formats hours', () => {
    expect(calculateETA(3600000, 1000)).toBe('1h 0m');
  });
});

describe('progressPercent', () => {
  it('returns 0 for zero total', () => {
    expect(progressPercent({ total_size: 0, downloaded_size: 50 })).toBe(0);
  });
  it('computes rounded percent', () => {
    expect(progressPercent({ total_size: 200, downloaded_size: 50 })).toBe(25);
  });
  it('caps at 100', () => {
    expect(progressPercent({ total_size: 100, downloaded_size: 150 })).toBe(100);
  });
});

describe('fileExtension', () => {
  it('uppercases the extension', () => {
    expect(fileExtension('video.mp4')).toBe('MP4');
  });
  it('returns empty for no extension', () => {
    expect(fileExtension('README')).toBe('');
  });
});

describe('fileFullPath', () => {
  it('joins save_path and filename', () => {
    expect(fileFullPath({ save_path: '/a/b', filename: 'c.mp4' })).toBe('/a/b/c.mp4');
  });
  it('strips trailing slash', () => {
    expect(fileFullPath({ save_path: '/a/b/', filename: 'c.mp4' })).toBe('/a/b/c.mp4');
  });
});
