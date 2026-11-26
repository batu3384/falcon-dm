import { describe, expect, it } from 'vitest';
import { parseDownloadEvent, parsePairRequest, parseProgressEvent, parseRemovedId } from './events';
import { makeRow } from '../test/fixtures';

describe('event payload guards', () => {
  it('accepts a well-formed download and drops a bad status', () => {
    const row = makeRow({ status: 'Downloading' });
    expect(parseDownloadEvent(row)?.id).toBe(row.id);
    expect(parseDownloadEvent({ ...row, status: 'Weird' })).toBeNull();
  });

  it('accepts progress and ignores malformed ticks', () => {
    const tick = {
      id: 1,
      downloaded_size: 100,
      total_size: 200,
      speed: 50,
      status: 'Downloading',
      connections: 8,
    };
    expect(parseProgressEvent(tick)?.speed).toBe(50);
    expect(parseProgressEvent({ ...tick, status: 'Weird' })).toBeNull();
    expect(parseProgressEvent({ ...tick, speed: -1 })).toBeNull();
  });

  it('requires a valid extension id and a six-digit code', () => {
    const id = 'abcdefghijklmnopabcdefghijklmnop';
    expect(parsePairRequest({ extension_id: id, code: '123456' })).toEqual({
      extension_id: id,
      code: '123456',
    });
    expect(parsePairRequest({ extension_id: id })).toBeNull();
    expect(parsePairRequest({ extension_id: 'short', code: '123456' })).toBeNull();
    expect(parsePairRequest(null)).toBeNull();
  });

  it('only accepts integer ids for removals', () => {
    expect(parseRemovedId(7)).toBe(7);
    expect(parseRemovedId('7')).toBeNull();
    expect(parseRemovedId(1.5)).toBeNull();
  });
});
