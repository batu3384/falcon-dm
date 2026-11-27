import { describe, expect, it } from 'vitest';
import { downloadRowShortcut } from './rowShortcuts';

function key(
  value: string,
  mods: Partial<Pick<KeyboardEvent, 'metaKey' | 'ctrlKey' | 'shiftKey' | 'altKey'>> = {},
) {
  return {
    key: value,
    metaKey: false,
    ctrlKey: false,
    shiftKey: false,
    altKey: false,
    ...mods,
  };
}

describe('downloadRowShortcut', () => {
  it('maps the row action shortcuts', () => {
    expect(downloadRowShortcut(key('R', { metaKey: true, shiftKey: true }))).toBe('reveal');
    expect(downloadRowShortcut(key('o', { metaKey: true }))).toBe('open');
    expect(downloadRowShortcut(key(' ', { ctrlKey: true }))).toBe('menu');
  });

  it('rejects incomplete or conflicting combinations', () => {
    expect(downloadRowShortcut(key('r', { metaKey: true }))).toBeNull();
    expect(downloadRowShortcut(key('o', { metaKey: true, shiftKey: true }))).toBeNull();
    expect(downloadRowShortcut(key(' ', { metaKey: true }))).toBeNull();
    expect(downloadRowShortcut(key(' ', { ctrlKey: true, shiftKey: true }))).toBeNull();
    expect(downloadRowShortcut(key('o', { metaKey: true, altKey: true }))).toBeNull();
  });
});
