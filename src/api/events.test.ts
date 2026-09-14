import { describe, expect, it } from 'vitest';
import { parseDownloadEvent, parsePairExtensionId, parseProgressEvent } from './events';

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

describe('event payload guards', () => {
  it('accepts a well-formed download and drops a bad status', () => {
    expect(parseDownloadEvent(validDownload)?.id).toBe(1);
    expect(parseDownloadEvent({ ...validDownload, status: 'Weird' })).toBeNull();
  });

  it('accepts progress and ignores malformed ticks', () => {
    expect(
      parseProgressEvent({
        id: 1,
        downloaded_size: 100,
        total_size: 200,
        speed: 50,
        status: 'Downloading',
        connections: 8,
      })?.speed,
    ).toBe(50);
    expect(
      parseProgressEvent({
        id: 1,
        downloaded_size: 100,
        total_size: 200,
        speed: 50,
        status: 'Weird',
        connections: 8,
      }),
    ).toBeNull();
  });

  it('requires a non-empty pair extension id', () => {
    expect(parsePairExtensionId({ extension_id: 'aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa' })).toBe(
      'aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa',
    );
    expect(parsePairExtensionId({ extension_id: '  ' })).toBeNull();
    expect(parsePairExtensionId({ extension_id: 1 })).toBeNull();
    expect(parsePairExtensionId(null)).toBeNull();
  });
});
