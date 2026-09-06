import { describe, expect, it } from 'vitest';
import { extractTauriError, localizeDownloadError } from './commands';

describe('extractTauriError', () => {
  it('maps missing invoke to a human message', () => {
    document.documentElement.lang = 'en';
    expect(
      extractTauriError(new TypeError("Cannot read properties of undefined (reading 'invoke')")),
    ).toBe('Falcon DM engine is not connected');
  });

  it('keeps normal backend strings', () => {
    expect(extractTauriError('invalid url')).toBe('invalid url');
  });

  it('localizeDownloadError maps magnet and invalid url codes', () => {
    const t = (key: string) =>
      ({
        'errors.unsupported_magnet': 'No magnet',
        'errors.invalid_url': 'Bad URL',
        'errors.junk_media_url': 'Junk',
      })[key] || key;
    expect(localizeDownloadError('ERR_UNSUPPORTED_MAGNET', t)).toBe('No magnet');
    expect(localizeDownloadError('invalid url', t)).toBe('Bad URL');
    expect(localizeDownloadError('not a real media url', t)).toBe('Junk');
  });
});
