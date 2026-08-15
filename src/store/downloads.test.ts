import { describe, it, expect, beforeEach, vi } from 'vitest';
import { useDownloadsStore } from './downloads';
import type { DownloadModel } from '../types';
import { getDownloads } from '../api/commands';

vi.mock('../api/commands', () => ({
  getDownloads: vi.fn(),
}));

const mockedGetDownloads = vi.mocked(getDownloads);

// ponytail: the downloads store drives the entire list UI. Test the optimistic
// update paths (add, progress, select) so a regression can't desync the list
// from the backend events.

const baseDownload: DownloadModel = {
  id: 1,
  url: 'https://example.com/a.mp4',
  filename: 'a.mp4',
  save_path: '/tmp',
  total_size: 1000,
  downloaded_size: 0,
  status: 'Queued',
  category: 'Video',
  speed: 0,
  segments: 8,
  priority: 1,
  created_at: '2026-08-01T00:00:00Z',
  completed_at: null,
  error_message: null,
  referrer: null,
  user_agent: null,
  aria2_gid: null,
  archived: false,
};

describe('useDownloadsStore', () => {
  beforeEach(() => {
    mockedGetDownloads.mockReset();
    useDownloadsStore.setState({
      downloads: [],
      loading: true,
      error: null,
      archived: false,
      search: '',
      loadedPages: 1,
      hasMore: false,
      loadingMore: false,
      requestSequence: 0,
      selectedDownload: null,
      selectedIds: new Set(),
      lastSelectId: null,
    });
  });

  it('addDownload prepends and dedupes by id', () => {
    useDownloadsStore.getState().addDownload(baseDownload);
    useDownloadsStore.getState().addDownload(baseDownload); // duplicate id
    expect(useDownloadsStore.getState().downloads).toHaveLength(1);
  });

  it('applyProgress updates the matching row only', () => {
    useDownloadsStore.getState().addDownload(baseDownload);
    useDownloadsStore.getState().addDownload({ ...baseDownload, id: 2 });
    useDownloadsStore.getState().applyProgress({
      id: 1,
      downloaded_size: 500,
      total_size: 1000,
      speed: 100,
      status: 'Downloading',
      connections: 8,
    });
    const list = useDownloadsStore.getState().downloads;
    expect(list.find((d) => d.id === 1)?.downloaded_size).toBe(500);
    expect(list.find((d) => d.id === 2)?.downloaded_size).toBe(0);
  });

  it('applyProgress ignores unknown ids', () => {
    useDownloadsStore.getState().addDownload(baseDownload);
    useDownloadsStore.getState().applyProgress({
      id: 999,
      downloaded_size: 1,
      total_size: 1,
      speed: 1,
      status: 'Downloading',
      connections: 1,
    });
    expect(useDownloadsStore.getState().downloads[0].downloaded_size).toBe(0);
  });

  it('selectDownload with no item clears selection', () => {
    useDownloadsStore.getState().addDownload(baseDownload);
    useDownloadsStore.getState().selectDownload(baseDownload);
    useDownloadsStore.getState().selectDownload(null);
    expect(useDownloadsStore.getState().selectedDownload).toBeNull();
    expect(useDownloadsStore.getState().selectedIds.size).toBe(0);
  });

  it('selectDownload plain click selects one', () => {
    useDownloadsStore.getState().addDownload(baseDownload);
    useDownloadsStore.getState().selectDownload(baseDownload);
    expect(useDownloadsStore.getState().selectedDownload?.id).toBe(1);
    expect(useDownloadsStore.getState().selectedIds.has(1)).toBe(true);
  });

  it('selectDownload meta toggles membership', () => {
    useDownloadsStore.getState().addDownload(baseDownload);
    useDownloadsStore.getState().addDownload({ ...baseDownload, id: 2 });
    useDownloadsStore.getState().selectDownload(baseDownload);
    useDownloadsStore.getState().selectDownload({ ...baseDownload, id: 2 }, { meta: true });
    expect(useDownloadsStore.getState().selectedIds.size).toBe(2);
    // toggle off
    useDownloadsStore.getState().selectDownload({ ...baseDownload, id: 2 }, { meta: true });
    expect(useDownloadsStore.getState().selectedIds.size).toBe(1);
  });

  it('clearSelection empties ids + selected', () => {
    useDownloadsStore.getState().addDownload(baseDownload);
    useDownloadsStore.getState().selectDownload(baseDownload);
    useDownloadsStore.getState().clearSelection();
    expect(useDownloadsStore.getState().selectedIds.size).toBe(0);
    expect(useDownloadsStore.getState().selectedDownload).toBeNull();
  });

  it('syncSelectedFromList clears a removed selection', () => {
    useDownloadsStore.getState().addDownload(baseDownload);
    useDownloadsStore.getState().selectDownload(baseDownload);
    // simulate removal
    useDownloadsStore.setState({ downloads: [] });
    useDownloadsStore.getState().syncSelectedFromList();
    expect(useDownloadsStore.getState().selectedDownload).toBeNull();
  });

  it('ignores an older response after a newer filter request', async () => {
    function deferred<T>() {
      let resolve!: (value: T) => void;
      const promise = new Promise<T>((next) => {
        resolve = next;
      });
      return { promise, resolve };
    }

    const first = deferred<DownloadModel[]>();
    const second = deferred<DownloadModel[]>();
    mockedGetDownloads.mockReturnValueOnce(first.promise).mockReturnValueOnce(second.promise);

    const a = useDownloadsStore.getState().fetchDownloads(false);
    const b = useDownloadsStore.getState().fetchDownloads(true);
    second.resolve([{ ...baseDownload, archived: true }]);
    first.resolve([baseDownload]);
    await Promise.all([a, b]);

    expect(useDownloadsStore.getState().downloads[0].archived).toBe(true);
  });

  it('retry preserves archived query', async () => {
    mockedGetDownloads.mockResolvedValueOnce([{ ...baseDownload, archived: true }]);
    await useDownloadsStore.getState().fetchDownloads(true);
    mockedGetDownloads.mockResolvedValueOnce([{ ...baseDownload, archived: true }]);

    await useDownloadsStore.getState().retryFetch();

    expect(mockedGetDownloads).toHaveBeenLastCalledWith({ archived: true });
  });

  it('sends search to backend so unloaded pages remain searchable', async () => {
    mockedGetDownloads.mockResolvedValueOnce([baseDownload]);

    await useDownloadsStore.getState().fetchDownloads(false, 'example');

    expect(mockedGetDownloads).toHaveBeenCalledWith({
      archived: false,
      search: 'example',
    });
  });

  it('stores fetch errors and clears them after a successful retry', async () => {
    mockedGetDownloads.mockRejectedValueOnce(new Error('offline'));
    await useDownloadsStore.getState().fetchDownloads();
    expect(useDownloadsStore.getState().error).toBe('offline');

    mockedGetDownloads.mockResolvedValueOnce([baseDownload]);
    await useDownloadsStore.getState().retryFetch();
    expect(useDownloadsStore.getState().error).toBeNull();
  });

  it('keeps existing rows visible while polling', async () => {
    useDownloadsStore.getState().addDownload(baseDownload);
    let resolve!: (value: DownloadModel[]) => void;
    mockedGetDownloads.mockReturnValueOnce(
      new Promise<DownloadModel[]>((next) => {
        resolve = next;
      }),
    );

    const refresh = useDownloadsStore.getState().fetchDownloads();
    expect(useDownloadsStore.getState().loading).toBe(false);
    resolve([baseDownload]);
    await refresh;
  });

  it('loads the next page without dropping current selection', async () => {
    useDownloadsStore.setState({
      downloads: [baseDownload],
      selectedDownload: baseDownload,
      selectedIds: new Set([baseDownload.id]),
      archived: false,
      loadedPages: 1,
      hasMore: true,
    });
    mockedGetDownloads.mockResolvedValueOnce([{ ...baseDownload, id: 2, filename: 'b.mp4' }]);

    await useDownloadsStore.getState().loadMoreDownloads();

    expect(mockedGetDownloads).toHaveBeenCalledWith({
      archived: false,
      limit: 200,
      before_id: 1,
    });
    expect(useDownloadsStore.getState().downloads.map((download) => download.id)).toEqual([1, 2]);
    expect(useDownloadsStore.getState().selectedIds.has(1)).toBe(true);
  });
});
