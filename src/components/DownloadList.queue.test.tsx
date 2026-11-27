import { beforeEach, describe, expect, it, vi } from 'vitest';
import { render, within } from '@testing-library/react';
import i18n from '../i18n';
import DownloadList from './DownloadList';
import { useDownloadsStore } from '../store/downloads';
import { makeRow } from '../test/fixtures';

vi.mock('@tanstack/react-virtual', () => ({
  useVirtualizer: ({ count, estimateSize }: { count: number; estimateSize: () => number }) => {
    const size = estimateSize();
    return {
      getVirtualItems: () =>
        Array.from({ length: count }, (_, index) => ({
          index,
          start: index * size,
          size,
          key: index,
        })),
      getTotalSize: () => count * size,
      scrollToIndex: () => {},
    };
  },
}));

describe('DownloadList queue positions', () => {
  beforeEach(() => {
    void i18n.changeLanguage('en');
    useDownloadsStore.setState({
      rows: [],
      view: 'all',
      search: '',
      loading: false,
      error: null,
      hasMore: false,
      selectedIds: new Set(),
      primaryId: null,
    });
  });

  it('shows the priority order on queued rows without reordering the list', () => {
    const older = makeRow({ id: 10, priority: 1, status: 'Queued' });
    const urgent = makeRow({ id: 20, priority: 4, status: 'Queued' });
    const newer = makeRow({ id: 30, priority: 1, status: 'Queued' });
    useDownloadsStore.setState({ rows: [newer, urgent, older] });

    render(<DownloadList />);
    for (const [id, position] of [
      [20, 1],
      [10, 2],
      [30, 3],
    ] as const) {
      const element = document.getElementById(`dl-row-${id}`);
      expect(element).not.toBeNull();
      expect(
        within(element as HTMLElement).getByTitle(`Queue position ${position}`),
      ).toHaveTextContent(`#${position}`);
    }
    expect(useDownloadsStore.getState().rows.map((row) => row.id)).toEqual([30, 20, 10]);
  });
});
