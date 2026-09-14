import { describe, expect, it } from 'vitest';
import { contextMenuNavIndex } from './DownloadItem';

describe('contextMenuNavIndex', () => {
  it('wraps arrows and jumps Home/End', () => {
    expect(contextMenuNavIndex(4, 0, 'ArrowDown')).toBe(1);
    expect(contextMenuNavIndex(4, 3, 'ArrowDown')).toBe(0);
    expect(contextMenuNavIndex(4, 0, 'ArrowUp')).toBe(3);
    expect(contextMenuNavIndex(4, 2, 'Home')).toBe(0);
    expect(contextMenuNavIndex(4, 0, 'End')).toBe(3);
  });

  it('treats missing focus as the first item', () => {
    expect(contextMenuNavIndex(3, -1, 'ArrowDown')).toBe(1);
    expect(contextMenuNavIndex(0, 0, 'ArrowDown')).toBeNull();
    expect(contextMenuNavIndex(3, 0, 'Tab')).toBeNull();
  });
});
