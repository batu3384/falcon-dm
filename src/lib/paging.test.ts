import { describe, expect, it, vi } from 'vitest';
import { collectPaged, nextPageBeforeId } from './paging';

describe('nextPageBeforeId', () => {
  it('stops when the page is short', () => {
    expect(nextPageBeforeId([{ id: 9 }, { id: 8 }], 200)).toBeUndefined();
  });

  it('returns the last id when the page is full', () => {
    const rows = Array.from({ length: 3 }, (_, i) => ({ id: 10 - i }));
    expect(nextPageBeforeId(rows, 3)).toBe(8);
  });
});

describe('collectPaged', () => {
  it('walks every page until a short one', async () => {
    const pages = [[{ id: 4 }, { id: 3 }], [{ id: 2 }, { id: 1 }], [{ id: 0 }]];
    const fetchPage = vi.fn(async (beforeId?: number) => {
      if (beforeId === undefined) return pages[0];
      if (beforeId === 3) return pages[1];
      return pages[2];
    });
    const rows = await collectPaged(2, fetchPage);
    expect(rows.map((row) => row.id)).toEqual([4, 3, 2, 1, 0]);
    expect(fetchPage).toHaveBeenCalledTimes(3);
  });
});
