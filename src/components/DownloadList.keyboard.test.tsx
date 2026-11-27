import { beforeEach, describe, expect, it, vi } from 'vitest';
import { fireEvent, render, screen } from '@testing-library/react';
import i18n from '../i18n';
import DownloadList from './DownloadList';
import { useDownloadsStore } from '../store/downloads';
import { makeRow } from '../test/fixtures';

const actions = vi.hoisted(() => ({
  pause: vi.fn(),
  resume: vi.fn(),
  toggle: vi.fn(),
  remove: vi.fn(),
  reveal: vi.fn(),
  open: vi.fn(),
  copyUrl: vi.fn(),
  archive: vi.fn(),
  priority: vi.fn(),
  moveRename: vi.fn(),
  redownload: vi.fn(),
}));

vi.mock('../lib/useDownloadActions', () => ({ useDownloadActions: () => actions }));

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

function seed(row = makeRow()) {
  useDownloadsStore.setState({
    rows: [row],
    view: 'all',
    search: '',
    loading: false,
    error: null,
    hasMore: false,
    selectedIds: new Set([row.id]),
    primaryId: row.id,
  });
  return row;
}

describe('DownloadList row shortcuts', () => {
  beforeEach(() => {
    void i18n.changeLanguage('en');
    Object.values(actions).forEach((fn) => fn.mockReset());
  });

  it('reveals the selected row with Cmd Shift R', () => {
    const row = seed();
    render(<DownloadList />);
    fireEvent.keyDown(screen.getByRole('listbox'), { key: 'R', metaKey: true, shiftKey: true });
    expect(actions.reveal).toHaveBeenCalledWith(row);
  });

  it('opens the selected completed row with Cmd O', () => {
    const row = seed(makeRow({ status: 'Completed', downloaded_size: 1000 }));
    render(<DownloadList />);
    fireEvent.keyDown(screen.getByRole('listbox'), { key: 'o', metaKey: true });
    expect(actions.open).toHaveBeenCalledWith(row);
  });

  it('opens the row context menu with Ctrl Space instead of toggling the row', async () => {
    seed();
    render(<DownloadList />);
    fireEvent.keyDown(screen.getByRole('listbox'), { key: ' ', ctrlKey: true });
    expect(actions.toggle).not.toHaveBeenCalled();
    expect(await screen.findByRole('menu')).toBeInTheDocument();
    expect(screen.getByRole('menuitem', { name: 'Show in Finder' })).toBeInTheDocument();
  });
});
