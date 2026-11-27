import { describe, it, expect, beforeEach, vi } from 'vitest';
import { render, fireEvent } from '@testing-library/react';
import '../i18n';
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

describe('DownloadList selection clicks', () => {
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

  it('clears the selection when the virtualized empty space is clicked', () => {
    const row = makeRow();
    useDownloadsStore.setState({
      rows: [row],
      selectedIds: new Set([row.id]),
      primaryId: row.id,
    });
    const { container } = render(<DownloadList />);
    const spacer = container.querySelector('.list-spacer');
    expect(spacer).not.toBeNull();

    fireEvent.mouseDown(spacer!);
    expect(useDownloadsStore.getState().selectedIds.size).toBe(0);
    expect(useDownloadsStore.getState().primaryId).toBeNull();
  });
});
