import { describe, expect, it } from 'vitest';
import { isQueueableClipboardUrl } from './useClipboardMonitor';

describe('isQueueableClipboardUrl', () => {
  it('accepts normal http(s) links', () => {
    expect(isQueueableClipboardUrl('https://example.com/file.zip')).toBe(true);
  });

  it('rejects junk and overlong urls', () => {
    expect(isQueueableClipboardUrl('https://x.com/no_input.mp3')).toBe(false);
    expect(isQueueableClipboardUrl(`https://x.com/${'a'.repeat(2100)}`)).toBe(false);
    expect(isQueueableClipboardUrl('not-a-url')).toBe(false);
    expect(isQueueableClipboardUrl('magnet:?xt=urn:btih:abc')).toBe(false);
  });
});
