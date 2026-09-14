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
      view: 'All Downloads',
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

  it('addDownload skips rows that do not match the current view', () => {
    useDownloadsStore.setState({ view: 'Archived', archived: true });
    useDownloadsStore.getState().addDownload(baseDownload);
    expect(useDownloadsStore.getState().downloads).toHaveLength(0);

    useDownloadsStore.setState({ view: 'Failed', archived: false });
    useDownloadsStore.getState().addDownload(baseDownload);
    expect(useDownloadsStore.getState().downloads).toHaveLength(0);

    useDownloadsStore.setState({ view: 'Downloading', archived: false });
    useDownloadsStore.getState().addDownload(baseDownload);
    expect(useDownloadsStore.getState().downloads).toHaveLength(1);

    useDownloadsStore.setState({ downloads: [], view: 'All Downloads', archived: false });
    useDownloadsStore.getState().addDownload(baseDownload);
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

  it('applyProgress copies Failed error_message and clears it on resume', () => {
    useDownloadsStore.setState({
      downloads: [{ ...baseDownload, status: 'Downloading', downloaded_size: 400 }],
    });
    useDownloadsStore.getState().applyProgress({
      id: 1,
      downloaded_size: 0,
      total_size: 0,
      speed: 0,
      status: 'Failed',
      connections: 0,
      error_message: 'HTTP 403',
    });
    expect(useDownloadsStore.getState().downloads[0].error_message).toBe('HTTP 403');
    useDownloadsStore.getState().applyProgress({
      id: 1,
      downloaded_size: 400,
      total_size: 1000,
      speed: 10,
      status: 'Downloading',
      connections: 8,
    });
    expect(useDownloadsStore.getState().downloads[0].error_message).toBeNull();
  });

  it('applyProgress keeps monotonic downloaded_size', () => {
    useDownloadsStore.setState({
      downloads: [{ ...baseDownload, downloaded_size: 800, status: 'Downloading' }],
    });
    useDownloadsStore.getState().applyProgress({
      id: 1,
      downloaded_size: 200,
      total_size: 1000,
      speed: 50,
      status: 'Downloading',
      connections: 8,
    });
    expect(useDownloadsStore.getState().downloads[0].downloaded_size).toBe(800);
  });

  it('applyProgress does not regress Completed', () => {
    useDownloadsStore.setState({
      downloads: [
        {
          ...baseDownload,
          downloaded_size: 1000,
          total_size: 1000,
          status: 'Completed',
          speed: 0,
        },
      ],
    });
    useDownloadsStore.getState().applyProgress({
      id: 1,
      downloaded_size: 900,
      total_size: 1000,
      speed: 40,
      status: 'Downloading',
      connections: 8,
    });
    const row = useDownloadsStore.getState().downloads[0];
    expect(row.status).toBe('Completed');
    expect(row.speed).toBe(0);
    expect(row.downloaded_size).toBe(1000);
  });

  it('applyProgress does not revive Paused from a stale Downloading tick', () => {
    useDownloadsStore.setState({
      downloads: [{ ...baseDownload, status: 'Paused', downloaded_size: 400, speed: 0 }],
    });
    useDownloadsStore.getState().applyProgress({
      id: 1,
      downloaded_size: 450,
      total_size: 1000,
      speed: 40,
      status: 'Downloading',
      connections: 8,
    });
    const paused = useDownloadsStore.getState().downloads[0];
    expect(paused.status).toBe('Paused');
    expect(paused.speed).toBe(0);
    expect(paused.downloaded_size).toBe(400);
  });

  it('applyProgress accepts Queued after Paused so tray/IPC resume is live', () => {
    useDownloadsStore.setState({
      downloads: [{ ...baseDownload, status: 'Paused', downloaded_size: 400, speed: 0 }],
    });
    useDownloadsStore.getState().applyProgress({
      id: 1,
      downloaded_size: 400,
      total_size: 1000,
      speed: 0,
      status: 'Queued',
      connections: 0,
    });
    expect(useDownloadsStore.getState().downloads[0].status).toBe('Queued');
    useDownloadsStore.getState().applyProgress({
      id: 1,
      downloaded_size: 410,
      total_size: 1000,
      speed: 20,
      status: 'Downloading',
      connections: 8,
    });
    const live = useDownloadsStore.getState().downloads[0];
    expect(live.status).toBe('Downloading');
    expect(live.downloaded_size).toBe(410);
  });

  it('applyProgress Queued demotes a live Downloading row', () => {
    useDownloadsStore.setState({
      downloads: [{ ...baseDownload, status: 'Downloading', downloaded_size: 400, speed: 80 }],
    });
    useDownloadsStore.getState().applyProgress({
      id: 1,
      downloaded_size: 400,
      total_size: 1000,
      speed: 0,
      status: 'Queued',
      connections: 0,
    });
    const row = useDownloadsStore.getState().downloads[0];
    expect(row.status).toBe('Queued');
    expect(row.speed).toBe(0);
  });

  it('applyProgress zeros speed when status leaves Downloading', () => {
    useDownloadsStore.setState({
      downloads: [{ ...baseDownload, status: 'Downloading', speed: 80, downloaded_size: 400 }],
    });
    useDownloadsStore.getState().applyProgress({
      id: 1,
      downloaded_size: 400,
      total_size: 1000,
      speed: 0,
      status: 'Merging',
      connections: 0,
    });
    expect(useDownloadsStore.getState().downloads[0].speed).toBe(0);
    expect(useDownloadsStore.getState().downloads[0].status).toBe('Merging');
    useDownloadsStore.getState().applyProgress({
      id: 1,
      downloaded_size: 400,
      total_size: 1000,
      speed: 12,
      status: 'Queued',
      connections: 0,
    });
    expect(useDownloadsStore.getState().downloads[0].speed).toBe(0);
    expect(useDownloadsStore.getState().downloads[0].status).toBe('Queued');
  });

  it('applyProgress lets Failed override a premature Completed tick', () => {
    useDownloadsStore.setState({
      downloads: [
        {
          ...baseDownload,
          downloaded_size: 1000,
          total_size: 1000,
          status: 'Completed',
          speed: 0,
        },
      ],
    });
    useDownloadsStore.getState().applyProgress({
      id: 1,
      downloaded_size: 1000,
      total_size: 1000,
      speed: 0,
      status: 'Failed',
      connections: 0,
    });
    expect(useDownloadsStore.getState().downloads[0].status).toBe('Failed');
  });

  it('applyProgress lets Failed become Downloading on retry', () => {
    useDownloadsStore.setState({
      downloads: [{ ...baseDownload, status: 'Failed', speed: 0, downloaded_size: 100 }],
    });
    useDownloadsStore.getState().applyProgress({
      id: 1,
      downloaded_size: 120,
      total_size: 1000,
      speed: 30,
      status: 'Downloading',
      connections: 4,
    });
    const row = useDownloadsStore.getState().downloads[0];
    expect(row.status).toBe('Downloading');
    expect(row.speed).toBe(30);
  });

  it('applyProgress uses current speed, not the peak', () => {
    useDownloadsStore.setState({
      downloads: [{ ...baseDownload, status: 'Downloading', speed: 400, downloaded_size: 100 }],
    });
    useDownloadsStore.getState().applyProgress({
      id: 1,
      downloaded_size: 200,
      total_size: 1000,
      speed: 50,
      status: 'Downloading',
      connections: 8,
    });
    expect(useDownloadsStore.getState().downloads[0].speed).toBe(50);
  });

  it('fetchDownloads preserves live progress when poll returns stale status', async () => {
    useDownloadsStore.setState({
      downloads: [{ ...baseDownload, downloaded_size: 600, status: 'Downloading', speed: 120 }],
    });
    mockedGetDownloads.mockResolvedValueOnce([
      { ...baseDownload, downloaded_size: 0, status: 'Queued', speed: 0 },
    ]);
    await useDownloadsStore.getState().fetchDownloads();
    const row = useDownloadsStore.getState().downloads[0];
    expect(row.status).toBe('Downloading');
    expect(row.downloaded_size).toBe(600);
    expect(row.speed).toBe(120);
  });

  it('fetchDownloads does not keep Downloading when backend completed', async () => {
    useDownloadsStore.setState({
      downloads: [{ ...baseDownload, downloaded_size: 900, status: 'Downloading', speed: 50 }],
    });
    mockedGetDownloads.mockResolvedValueOnce([
      {
        ...baseDownload,
        downloaded_size: 1000,
        total_size: 1000,
        status: 'Completed',
        speed: 0,
        completed_at: '2026-08-01T00:01:00Z',
      },
    ]);
    await useDownloadsStore.getState().fetchDownloads();
    const row = useDownloadsStore.getState().downloads[0];
    expect(row.status).toBe('Completed');
    expect(row.downloaded_size).toBe(1000);
    expect(row.speed).toBe(0);
  });

  it('fetchDownloads does not revive Completed from a stale Downloading poll', async () => {
    useDownloadsStore.setState({
      downloads: [
        {
          ...baseDownload,
          downloaded_size: 1000,
          total_size: 1000,
          status: 'Completed',
          speed: 0,
        },
      ],
    });
    mockedGetDownloads.mockResolvedValueOnce([
      { ...baseDownload, downloaded_size: 900, status: 'Downloading', speed: 40 },
    ]);
    await useDownloadsStore.getState().fetchDownloads();
    const row = useDownloadsStore.getState().downloads[0];
    expect(row.status).toBe('Completed');
    expect(row.speed).toBe(0);
    expect(row.downloaded_size).toBe(1000);
  });

  it('poll Queued replaces stale Downloading', async () => {
    useDownloadsStore.setState({
      downloads: [{ ...baseDownload, status: 'Downloading', downloaded_size: 100, speed: 40 }],
    });
    mockedGetDownloads.mockResolvedValueOnce([
      { ...baseDownload, status: 'Queued', downloaded_size: 100, speed: 0 },
    ]);
    await useDownloadsStore.getState().fetchDownloads();
    expect(useDownloadsStore.getState().downloads[0].status).toBe('Queued');
  });

  it('poll does not revive Paused from a stale Downloading snapshot', async () => {
    useDownloadsStore.setState({
      downloads: [{ ...baseDownload, status: 'Paused', downloaded_size: 400, speed: 0 }],
    });
    mockedGetDownloads.mockResolvedValueOnce([
      { ...baseDownload, status: 'Downloading', downloaded_size: 450, speed: 40 },
    ]);
    await useDownloadsStore.getState().fetchDownloads();
    const paused = useDownloadsStore.getState().downloads[0];
    expect(paused.status).toBe('Paused');
    expect(paused.speed).toBe(0);
    expect(paused.downloaded_size).toBe(400);
  });

  it('poll Queued after Paused is live resume', async () => {
    useDownloadsStore.setState({
      downloads: [{ ...baseDownload, status: 'Paused', downloaded_size: 400, speed: 0 }],
    });
    mockedGetDownloads.mockResolvedValueOnce([
      { ...baseDownload, status: 'Queued', downloaded_size: 400, speed: 0 },
    ]);
    await useDownloadsStore.getState().fetchDownloads();
    expect(useDownloadsStore.getState().downloads[0].status).toBe('Queued');
  });

  it('applyProgress does not rewind Merging to a stale Downloading tick', () => {
    useDownloadsStore.setState({
      downloads: [{ ...baseDownload, status: 'Merging', downloaded_size: 1000, speed: 0 }],
    });
    useDownloadsStore.getState().applyProgress({
      id: 1,
      downloaded_size: 900,
      total_size: 1000,
      speed: 40,
      status: 'Downloading',
      connections: 8,
    });
    const row = useDownloadsStore.getState().downloads[0];
    expect(row.status).toBe('Merging');
    expect(row.speed).toBe(0);
    expect(row.downloaded_size).toBe(1000);
  });

  it('poll does not rewind Merging to a stale Downloading snapshot', async () => {
    useDownloadsStore.setState({
      downloads: [{ ...baseDownload, status: 'Merging', downloaded_size: 1000, speed: 0 }],
    });
    mockedGetDownloads.mockResolvedValueOnce([
      { ...baseDownload, status: 'Downloading', downloaded_size: 900, speed: 40 },
    ]);
    await useDownloadsStore.getState().fetchDownloads();
    const row = useDownloadsStore.getState().downloads[0];
    expect(row.status).toBe('Merging');
    expect(row.speed).toBe(0);
    expect(row.downloaded_size).toBe(1000);
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
    expect(useDownloadsStore.getState().selectedDownload?.id).toBe(2);
    // toggle off the last-clicked row — inspector stays on the remaining id
    useDownloadsStore.getState().selectDownload({ ...baseDownload, id: 2 }, { meta: true });
    expect(useDownloadsStore.getState().selectedIds.size).toBe(1);
    expect([...useDownloadsStore.getState().selectedIds]).toEqual([1]);
    expect(useDownloadsStore.getState().selectedDownload?.id).toBe(1);
    useDownloadsStore.getState().selectDownload(baseDownload, { meta: true });
    expect(useDownloadsStore.getState().selectedIds.size).toBe(0);
    expect(useDownloadsStore.getState().selectedDownload).toBeNull();
  });

  it('shift-select uses visible order instead of the unfiltered list', () => {
    const video = baseDownload;
    const music = { ...baseDownload, id: 2, filename: 'b.mp3', category: 'Music' as const };
    const video2 = { ...baseDownload, id: 3, filename: 'c.mp4' };
    useDownloadsStore.setState({ downloads: [video, music, video2] });
    useDownloadsStore.getState().selectDownload(video);
    useDownloadsStore.getState().selectDownload(video2, { shift: true }, [1, 3]);
    expect([...useDownloadsStore.getState().selectedIds].sort()).toEqual([1, 3]);
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

  it('sends Failed/Video filters to the backend', async () => {
    mockedGetDownloads.mockResolvedValueOnce([baseDownload]);
    await useDownloadsStore.getState().fetchDownloads(false, '', 'Failed');
    expect(mockedGetDownloads).toHaveBeenCalledWith({
      archived: false,
      status: 'Failed',
    });
    mockedGetDownloads.mockResolvedValueOnce([baseDownload]);
    await useDownloadsStore.getState().fetchDownloads(false, '', 'Video');
    expect(mockedGetDownloads).toHaveBeenLastCalledWith({
      archived: false,
      category: 'Video',
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
