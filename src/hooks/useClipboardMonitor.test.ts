import { describe, expect, it } from 'vitest';
import { isQueueableClipboardUrl, clipboardUrlConsumed } from './useClipboardMonitor';

describe('isQueueableClipboardUrl', () => {
  it('accepts normal http(s) links', () => {
    expect(isQueueableClipboardUrl('https://example.com/file.zip')).toBe(true);
  });

  it('rejects junk and overlong urls', () => {
    expect(isQueueableClipboardUrl('https://x.com/no_input.mp3')).toBe(false);
    expect(isQueueableClipboardUrl(`https://example.com/${'a'.repeat(3000)}`)).toBe(true);
    expect(isQueueableClipboardUrl(`https://example.com/${'a'.repeat(8200)}`)).toBe(false);
    expect(isQueueableClipboardUrl('not-a-url')).toBe(false);
    expect(isQueueableClipboardUrl('magnet:?xt=urn:btih:abc')).toBe(false);
  });

  it('rejects loopback clipboard targets', () => {
    expect(isQueueableClipboardUrl('http://127.0.0.1/file.bin')).toBe(false);
    expect(isQueueableClipboardUrl('http://192.168.0.5/a.zip')).toBe(false);
    expect(isQueueableClipboardUrl('https://ad.doubleclick.net/track')).toBe(false);
    expect(isQueueableClipboardUrl('https://cdn.example.com/doubleclick-audit.pdf')).toBe(true);
  });

  it('host-gates YouTube UI junk the same way as isJunkUrl', () => {
    expect(isQueueableClipboardUrl('https://cdn.example.com/api/log_event/export.csv')).toBe(
      true,
    );
    expect(isQueueableClipboardUrl('https://docs.example.com/s/search/guide.pdf')).toBe(true);
    expect(isQueueableClipboardUrl('https://www.youtube.com/api/stats')).toBe(false);
    expect(isQueueableClipboardUrl('https://www.youtube.com/ptracking?event=1')).toBe(false);
  });
});

describe('clipboardUrlConsumed', () => {
  it('keeps the URL eligible when the overlay callback returns false', () => {
    expect(clipboardUrlConsumed(false)).toBe(false);
    expect(clipboardUrlConsumed(undefined)).toBe(true);
    expect(clipboardUrlConsumed(true)).toBe(true);
  });
});
