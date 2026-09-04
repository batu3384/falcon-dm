import { describe, expect, it } from 'vitest';
import { extractTauriError } from './commands';

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
});
