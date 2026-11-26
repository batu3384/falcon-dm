import { describe, expect, it } from 'vitest';
import { retryCountdown, rowStatusLine } from './downloadText';
import { makeRow } from '../test/fixtures';

const t = (key: string, options?: Record<string, unknown>) =>
  options ? `${key}${JSON.stringify(options)}` : key;
const MB = 1024 * 1024;

describe('rowStatusLine', () => {
  it('shows bytes, speed and time left while downloading', () => {
    const row = makeRow({
      status: 'Downloading',
      total_size: 100 * MB,
      downloaded_size: 40 * MB,
      speed: 2 * MB,
    });
    const line = rowStatusLine(row, t, 'en');
    expect(line.text).toContain('40 MB / 100 MB');
    expect(line.text).toContain('2 MB/s');
    expect(line.text).toContain('row.remaining{"time":"30s"}');
    expect(line.tone).toBeUndefined();
  });

  it('says connecting before the first byte', () => {
    const line = rowStatusLine(makeRow({ status: 'Downloading', total_size: 0 }), t, 'en');
    expect(line.text.startsWith('row.connecting')).toBe(true);
  });

  it('counts down a pending automatic retry', () => {
    const now = 1_000_000;
    const row = makeRow({ status: 'Queued', retry_at: now + 9_500, attempts: 2 });
    expect(retryCountdown(row, now)).toBe(10);
    const line = rowStatusLine(row, t, 'en', now);
    expect(line.tone).toBe('warn');
    expect(line.text).toContain('"attempt":2');
    expect(retryCountdown(makeRow({ status: 'Paused', retry_at: now }), now)).toBeNull();
  });

  it('shows the localized reason of a failure', () => {
    const line = rowStatusLine(makeRow({ status: 'Failed', error_message: 'HTTP 404' }), t, 'en');
    expect(line).toEqual({ text: 'errors.http_not_found{"code":404}', tone: 'bad' });
    expect(rowStatusLine(makeRow({ status: 'Failed' }), t, 'en').text).toBe('status.failed');
  });

  it('keeps progress visible on paused rows', () => {
    const line = rowStatusLine(
      makeRow({ status: 'Paused', total_size: 10 * MB, downloaded_size: 5 * MB }),
      t,
      'en',
    );
    expect(line.text).toBe('status.paused · 5 MB / 10 MB · row.percent{"value":50}');
  });
});
