import { describe, it, expect, beforeEach, vi } from 'vitest';
import {
  applyProgressToRow,
  mergeFetchedRow,
  totalLiveSpeed,
  useDownloadsStore,
  DOWNLOAD_PAGE_SIZE,
} from './downloads';
import { getDownloads } from '../api/commands';
import { makeRow } from '../test/fixtures';

vi.mock('../api/commands', () => ({ getDownloads: vi.fn() }));
const mockedGetDownloads = vi.mocked(getDownloads);

const tick = (id: number, over: Partial<Parameters<typeof applyProgressToRow>[1]> = {}) => ({
  id,
  downloaded_size: 500,
  total_size: 1000,
  speed: 250,
  status: 'Downloading' as const,
  connections: 4,
  ...over,
});

function reset() {
  useDownloadsStore.setState({
    rows: [],
    view: 'all',
    search: '',
    loading: false,
    error: null,
    hasMore: false,
    loadingMore: false,
    loadedPages: 1,
    sequence: 0,
    selectedIds: new Set(),
    primaryId: null,
    liveSpeeds: {},
    byteRestartIds: new Set(),
  });
}

describe('row merge rules', () => {
  it('a late progress tick cannot revive a paused or finished row', () => {
    for (const status of ['Paused', 'Completed', 'Failed', 'Queued'] as const) {
      const row = makeRow({ status, downloaded_size: 100 });
      expect(applyProgressToRow(row, tick(row.id))).toBe(row);
    }
  });

  it('progress moves a live row and never rewinds Merging to Downloading', () => {
    const live = makeRow({ status: 'Downloading' });
    expect(applyProgressToRow(live, tick(live.id))).toMatchObject({
      downloaded_size: 500,
      speed: 250,
      segments: 4,
    });
    const merging = makeRow({ status: 'Merging' });
    expect(applyProgressToRow(merging, tick(merging.id)).status).toBe('Merging');
    expect(applyProgressToRow(live, tick(live.id, { status: 'Merging' }))).toMatchObject({
      status: 'Merging',
      speed: 0,
    });
  });

  it('a refetch keeps fresher live counters but takes any status change', () => {
    const existing = makeRow({ status: 'Downloading', downloaded_size: 900, speed: 50 });
    const stale = { ...existing, downloaded_size: 400, speed: 10, filename: 'renamed.bin' };
    expect(mergeFetchedRow(stale, existing)).toMatchObject({
      downloaded_size: 900,
      speed: 50,
      filename: 'renamed.bin',
    });
    const paused = { ...existing, status: 'Paused' as const, downloaded_size: 880 };
    expect(mergeFetchedRow(paused, existing)).toBe(paused);
  });

  it('sums only positive speeds', () => {
    expect(totalLiveSpeed({ 1: 100, 2: 0, 3: 50 })).toBe(150);
  });
});

describe('useDownloadsStore', () => {
  beforeEach(() => {
    mockedGetDownloads.mockReset();
    reset();
  });

  it('refresh loads the view filter and seeds live speeds', async () => {
    const live = makeRow({ status: 'Downloading', speed: 300 });
    const done = makeRow({ status: 'Completed' });
    mockedGetDownloads.mockResolvedValueOnce([live, done]);
    await useDownloadsStore.getState().refresh();
    expect(mockedGetDownloads).toHaveBeenCalledWith(
      expect.objectContaining({ archived: false, limit: DOWNLOAD_PAGE_SIZE }),
    );
    const state = useDownloadsStore.getState();
    expect(state.rows.map((r) => r.id)).toEqual([live.id, done.id]);
    expect(state.liveSpeeds).toEqual({ [live.id]: 300 });
    expect(state.loading).toBe(false);
  });

  it('switching views resets selection and fetches with the new filter', async () => {
    const row = makeRow({ status: 'Failed' });
    useDownloadsStore.setState({ rows: [row], selectedIds: new Set([row.id]), primaryId: row.id });
    mockedGetDownloads.mockResolvedValueOnce([row]);
    await useDownloadsStore.getState().setView('failed');
    expect(mockedGetDownloads).toHaveBeenCalledWith(
      expect.objectContaining({ status: 'Failed', archived: false }),
    );
    expect(useDownloadsStore.getState().selectedIds.size).toBe(0);
  });

  it('a slow older response cannot overwrite a newer one', async () => {
    const older = makeRow({ filename: 'older.bin' });
    const newer = makeRow({ filename: 'newer.bin' });
    let release: (rows: (typeof older)[]) => void = () => {};
    mockedGetDownloads.mockImplementationOnce(() => new Promise((resolve) => (release = resolve)));
    mockedGetDownloads.mockResolvedValueOnce([newer]);
    const first = useDownloadsStore.getState().refresh();
    await useDownloadsStore.getState().setSearch('newer');
    release([older]);
    await first;
    expect(useDownloadsStore.getState().rows.map((r) => r.filename)).toEqual(['newer.bin']);
  });

  it('keeps errors visible without dropping loaded rows', async () => {
    const row = makeRow();
    useDownloadsStore.setState({ rows: [row] });
    mockedGetDownloads.mockRejectedValueOnce(new Error('engine down'));
    await useDownloadsStore.getState().refresh();
    expect(useDownloadsStore.getState()).toMatchObject({ error: 'engine down', loading: false });
    expect(useDownloadsStore.getState().rows).toHaveLength(1);
  });

  it('loadMore appends the next cursor page once', async () => {
    const rows = Array.from({ length: DOWNLOAD_PAGE_SIZE }, (_, i) => makeRow({ id: 10_000 - i }));
    useDownloadsStore.setState({ rows, hasMore: true });
    const next = makeRow({ id: 1 });
    mockedGetDownloads.mockResolvedValueOnce([next, rows[0]]);
    await useDownloadsStore.getState().loadMore();
    expect(mockedGetDownloads).toHaveBeenCalledWith(
      expect.objectContaining({ before_id: rows[rows.length - 1].id }),
    );
    const state = useDownloadsStore.getState();
    expect(state.rows).toHaveLength(DOWNLOAD_PAGE_SIZE + 1);
    expect(state.hasMore).toBe(false);
  });

  it('upsert inserts new rows in id order and replaces known ones', () => {
    const a = makeRow({ id: 5 });
    const b = makeRow({ id: 3 });
    const store = useDownloadsStore.getState();
    store.upsert(b);
    store.upsert(a);
    store.upsert(makeRow({ id: 4 }));
    expect(useDownloadsStore.getState().rows.map((r) => r.id)).toEqual([5, 4, 3]);
    store.upsert({ ...b, filename: 'renamed.bin' });
    expect(useDownloadsStore.getState().rows.find((r) => r.id === 3)?.filename).toBe('renamed.bin');
  });

  it('upsert drops a row that left the view and its selection', () => {
    const row = makeRow({ status: 'Downloading' });
    useDownloadsStore.setState({
      view: 'active',
      rows: [row],
      selectedIds: new Set([row.id]),
      primaryId: row.id,
      liveSpeeds: { [row.id]: 100 },
    });
    useDownloadsStore.getState().upsert({ ...row, status: 'Completed' });
    const state = useDownloadsStore.getState();
    expect(state.rows).toHaveLength(0);
    expect(state.selectedIds.size).toBe(0);
    expect(state.primaryId).toBeNull();
    expect(state.liveSpeeds).toEqual({});
  });

  it('upsert respects the search filter and keeps fresher live bytes', () => {
    useDownloadsStore.setState({ search: 'movie' });
    useDownloadsStore.getState().upsert(makeRow({ filename: 'song.mp3' }));
    expect(useDownloadsStore.getState().rows).toHaveLength(0);
    const live = makeRow({ filename: 'movie.mp4', status: 'Downloading', downloaded_size: 800 });
    useDownloadsStore.setState({ rows: [live] });
    useDownloadsStore.getState().upsert({ ...live, downloaded_size: 300, segments: 8 });
    expect(useDownloadsStore.getState().rows[0]).toMatchObject({
      downloaded_size: 800,
      segments: 8,
    });
  });

  it('accepts lower bytes when a new run goes back to Downloading', () => {
    const merging = makeRow({ status: 'Merging', downloaded_size: 800 });
    useDownloadsStore.setState({ rows: [merging] });
    useDownloadsStore
      .getState()
      .upsert({ ...merging, status: 'Downloading', downloaded_size: 0, speed: 20 });
    expect(useDownloadsStore.getState().rows[0]).toMatchObject({
      status: 'Downloading',
      downloaded_size: 0,
    });
  });

  it('does not clamp restarted live bytes back to their old maximum', () => {
    const live = makeRow({ status: 'Downloading', downloaded_size: 800 });
    useDownloadsStore.setState({ rows: [live] });
    useDownloadsStore.getState().applyProgress(tick(live.id, { downloaded_size: 0, speed: 5 }));
    expect(useDownloadsStore.getState().rows[0].downloaded_size).toBe(0);

    useDownloadsStore.getState().upsert({ ...live, filename: 'renamed.bin' });
    expect(useDownloadsStore.getState().rows[0]).toMatchObject({
      filename: 'renamed.bin',
      downloaded_size: 0,
    });
  });

  it('progress for a row outside the view still counts towards the total speed', () => {
    useDownloadsStore.getState().applyProgress(tick(42, { speed: 700 }));
    expect(useDownloadsStore.getState().liveSpeeds).toEqual({ 42: 700 });
    const paused = makeRow({ status: 'Paused' });
    useDownloadsStore.setState({ rows: [paused] });
    useDownloadsStore.getState().applyProgress(tick(paused.id));
    expect(useDownloadsStore.getState().rows[0]).toBe(paused);
    expect(useDownloadsStore.getState().liveSpeeds[paused.id]).toBeUndefined();
  });

  it('remove clears the row, its speed and its selection', () => {
    const row = makeRow({ status: 'Downloading' });
    useDownloadsStore.setState({
      rows: [row],
      selectedIds: new Set([row.id]),
      primaryId: row.id,
      liveSpeeds: { [row.id]: 5 },
    });
    useDownloadsStore.getState().remove(row.id);
    expect(useDownloadsStore.getState()).toMatchObject({
      rows: [],
      primaryId: null,
      liveSpeeds: {},
    });
  });

  it('selection supports single, toggle, range and select-all', () => {
    const rows = [makeRow({ id: 9 }), makeRow({ id: 8 }), makeRow({ id: 7 }), makeRow({ id: 6 })];
    useDownloadsStore.setState({ rows });
    const store = useDownloadsStore.getState();
    store.select(8);
    expect([...useDownloadsStore.getState().selectedIds]).toEqual([8]);
    store.select(6, { range: true });
    expect([...useDownloadsStore.getState().selectedIds]).toEqual([8, 7, 6]);
    expect(useDownloadsStore.getState().primaryId).toBe(8);
    store.select(7, { toggle: true });
    expect([...useDownloadsStore.getState().selectedIds].sort()).toEqual([6, 8]);
    store.select(8, { toggle: true });
    expect(useDownloadsStore.getState().primaryId).toBe(6);
    store.selectAll();
    expect(useDownloadsStore.getState().selectedIds.size).toBe(4);
    store.select(null);
    expect(useDownloadsStore.getState()).toMatchObject({ primaryId: null });
    expect(useDownloadsStore.getState().selectedIds.size).toBe(0);
  });
});
