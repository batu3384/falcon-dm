import { create } from 'zustand';
import type { DownloadModel, ProgressPayload } from '../types';
import { getDownloads } from '../api/commands';
import {
  downloadMatchesSearch,
  downloadMatchesView,
  viewToDownloadFilter,
  type ViewId,
} from '../lib/downloadView';
import { isLive } from '../lib/downloadCapabilities';

export const DOWNLOAD_PAGE_SIZE = 200;

/**
 * A DB row from a refetch vs the row the store already holds. While both are
 * live, the store's counters are fresher (progress events arrive twice a
 * second, the DB is written every two). Anything else: the DB wins.
 */
export function mergeFetchedRow(fresh: DownloadModel, existing?: DownloadModel): DownloadModel {
  if (!existing || !isLive(fresh.status) || !isLive(existing.status)) return fresh;
  return {
    ...fresh,
    status: existing.status === 'Merging' ? 'Merging' : fresh.status,
    downloaded_size: existing.downloaded_size,
    total_size: existing.total_size || fresh.total_size,
    speed: existing.speed,
    segments: existing.segments,
  };
}

/** Counters from a running worker. Only a live row moves; a late tick cannot revive a stopped one. */
export function applyProgressToRow(row: DownloadModel, p: ProgressPayload): DownloadModel {
  if (!isLive(row.status)) return row;
  return {
    ...row,
    // Merging never goes back to Downloading within one run.
    status: row.status === 'Merging' ? 'Merging' : p.status,
    downloaded_size: p.downloaded_size,
    total_size: p.total_size || row.total_size,
    speed: p.status === 'Merging' ? 0 : p.speed,
    segments: p.connections || row.segments,
  };
}

function insertById(rows: DownloadModel[], row: DownloadModel): DownloadModel[] {
  // Rows are ordered newest first (id DESC), like the backend query.
  const index = rows.findIndex((existing) => existing.id < row.id);
  if (index === -1) return [...rows, row];
  return [...rows.slice(0, index), row, ...rows.slice(index)];
}

export interface SelectMods {
  toggle?: boolean;
  range?: boolean;
}

interface DownloadsState {
  rows: DownloadModel[];
  view: ViewId;
  search: string;
  loading: boolean;
  error: string | null;
  hasMore: boolean;
  loadingMore: boolean;
  loadedPages: number;
  sequence: number;
  selectedIds: Set<number>;
  /** Row shown in the inspector and used as the range-select anchor. */
  primaryId: number | null;
  /** Speed of every running download, whatever the current view shows. */
  liveSpeeds: Record<number, number>;

  setView: (view: ViewId) => Promise<void>;
  setSearch: (search: string) => Promise<void>;
  refresh: () => Promise<void>;
  loadMore: () => Promise<void>;
  upsert: (row: DownloadModel) => void;
  remove: (id: number) => void;
  applyProgress: (p: ProgressPayload) => void;
  select: (id: number | null, mods?: SelectMods) => void;
  selectAll: () => void;
  clearSelection: () => void;
}

export const useDownloadsStore = create<DownloadsState>((set, get) => {
  const fetchPages = async (pages: number, reset: boolean) => {
    const sequence = get().sequence + 1;
    const { view, search } = get();
    set({
      sequence,
      loadingMore: false,
      loading: reset || get().rows.length === 0,
      error: null,
      ...(reset ? { rows: [], hasMore: false } : {}),
    });
    try {
      const filter = {
        ...viewToDownloadFilter(view),
        ...(search.trim() ? { search: search.trim() } : {}),
      };
      const collected: DownloadModel[] = [];
      let beforeId: number | undefined;
      let lastPageFull = false;
      for (let page = 0; page < pages; page += 1) {
        const rows = await getDownloads({
          ...filter,
          limit: DOWNLOAD_PAGE_SIZE,
          ...(beforeId === undefined ? {} : { before_id: beforeId }),
        });
        collected.push(...rows);
        lastPageFull = rows.length === DOWNLOAD_PAGE_SIZE;
        beforeId = rows[rows.length - 1]?.id;
        if (!lastPageFull) break;
      }
      if (get().sequence !== sequence) return;
      set((state) => {
        const byId = new Map(state.rows.map((row) => [row.id, row]));
        const rows = collected.map((row) => mergeFetchedRow(row, byId.get(row.id)));
        const visible = new Set(rows.map((row) => row.id));
        const selectedIds = new Set([...state.selectedIds].filter((id) => visible.has(id)));
        // Seed speeds of live rows so the total is right before the first tick.
        const liveSpeeds = { ...state.liveSpeeds };
        for (const row of rows) {
          if (!isLive(row.status)) delete liveSpeeds[row.id];
          else if (!(row.id in liveSpeeds)) liveSpeeds[row.id] = row.speed;
        }
        return {
          rows,
          liveSpeeds,
          loading: false,
          hasMore: lastPageFull,
          loadedPages: Math.max(1, Math.ceil(rows.length / DOWNLOAD_PAGE_SIZE)),
          selectedIds,
          primaryId:
            state.primaryId !== null && visible.has(state.primaryId) ? state.primaryId : null,
        };
      });
    } catch (e) {
      if (get().sequence !== sequence) return;
      set({ loading: false, error: e instanceof Error ? e.message : String(e) });
    }
  };

  return {
    rows: [],
    view: 'all',
    search: '',
    loading: true,
    error: null,
    hasMore: false,
    loadingMore: false,
    loadedPages: 1,
    sequence: 0,
    selectedIds: new Set(),
    primaryId: null,
    liveSpeeds: {},

    setView: async (view) => {
      if (view === get().view) return;
      set({ view, selectedIds: new Set(), primaryId: null });
      await fetchPages(1, true);
    },

    setSearch: async (search) => {
      if (search === get().search) return;
      set({ search });
      await fetchPages(1, false);
    },

    refresh: () => fetchPages(Math.max(1, get().loadedPages), false),

    loadMore: async () => {
      const state = get();
      if (state.loadingMore || !state.hasMore || state.loading) return;
      const sequence = state.sequence + 1;
      set({ loadingMore: true, sequence });
      try {
        const rows = await getDownloads({
          ...viewToDownloadFilter(state.view),
          ...(state.search.trim() ? { search: state.search.trim() } : {}),
          limit: DOWNLOAD_PAGE_SIZE,
          before_id: state.rows[state.rows.length - 1]?.id,
        });
        if (get().sequence !== sequence) return;
        set((current) => {
          const known = new Set(current.rows.map((row) => row.id));
          return {
            rows: [...current.rows, ...rows.filter((row) => !known.has(row.id))],
            hasMore: rows.length === DOWNLOAD_PAGE_SIZE,
            loadedPages: current.loadedPages + 1,
            loadingMore: false,
          };
        });
      } catch (e) {
        if (get().sequence !== sequence) return;
        set({ loadingMore: false, error: e instanceof Error ? e.message : String(e) });
      }
    },

    upsert: (row) =>
      set((state) => {
        const liveSpeeds = { ...state.liveSpeeds };
        if (isLive(row.status)) liveSpeeds[row.id] = row.speed;
        else delete liveSpeeds[row.id];
        const index = state.rows.findIndex((existing) => existing.id === row.id);
        const belongs =
          downloadMatchesView(row, state.view) && downloadMatchesSearch(row, state.search);
        if (!belongs) {
          if (index === -1) return { liveSpeeds };
          const selectedIds = new Set(state.selectedIds);
          selectedIds.delete(row.id);
          return {
            liveSpeeds,
            rows: state.rows.filter((existing) => existing.id !== row.id),
            selectedIds,
            primaryId: state.primaryId === row.id ? null : state.primaryId,
          };
        }
        if (index === -1) return { liveSpeeds, rows: insertById(state.rows, row) };
        const existing = state.rows[index];
        // A row update never carries fresher counters than live progress.
        const next =
          isLive(row.status) && isLive(existing.status)
            ? {
                ...row,
                downloaded_size: Math.max(row.downloaded_size, existing.downloaded_size),
                speed: existing.speed,
              }
            : row;
        const rows = state.rows.slice();
        rows[index] = next;
        return { liveSpeeds, rows };
      }),

    remove: (id) =>
      set((state) => {
        const liveSpeeds = { ...state.liveSpeeds };
        delete liveSpeeds[id];
        const selectedIds = new Set(state.selectedIds);
        selectedIds.delete(id);
        return {
          liveSpeeds,
          rows: state.rows.filter((row) => row.id !== id),
          selectedIds,
          primaryId: state.primaryId === id ? null : state.primaryId,
        };
      }),

    applyProgress: (p) =>
      set((state) => {
        const index = state.rows.findIndex((row) => row.id === p.id);
        const current = index === -1 ? undefined : state.rows[index];
        // Speeds of rows outside this view still count towards the total.
        if (current && !isLive(current.status)) return state;
        const liveSpeeds = { ...state.liveSpeeds, [p.id]: p.status === 'Merging' ? 0 : p.speed };
        if (!current) return { liveSpeeds };
        const rows = state.rows.slice();
        rows[index] = applyProgressToRow(current, p);
        return { liveSpeeds, rows };
      }),

    select: (id, mods) =>
      set((state) => {
        if (id === null) return { selectedIds: new Set(), primaryId: null };
        if (mods?.toggle) {
          const selectedIds = new Set(state.selectedIds);
          if (selectedIds.has(id)) {
            selectedIds.delete(id);
            const primaryId =
              state.primaryId === id ? ([...selectedIds].pop() ?? null) : state.primaryId;
            return { selectedIds, primaryId };
          }
          selectedIds.add(id);
          return { selectedIds, primaryId: id };
        }
        if (mods?.range && state.primaryId !== null) {
          const ids = state.rows.map((row) => row.id);
          const from = ids.indexOf(state.primaryId);
          const to = ids.indexOf(id);
          if (from !== -1 && to !== -1) {
            const [lo, hi] = from < to ? [from, to] : [to, from];
            // The anchor stays put so the range can grow or shrink.
            return { selectedIds: new Set(ids.slice(lo, hi + 1)) };
          }
        }
        return { selectedIds: new Set([id]), primaryId: id };
      }),

    selectAll: () =>
      set((state) => ({
        selectedIds: new Set(state.rows.map((row) => row.id)),
        primaryId: state.primaryId ?? state.rows[0]?.id ?? null,
      })),

    clearSelection: () => set({ selectedIds: new Set(), primaryId: null }),
  };
});

/** Total speed of every running download. */
export function totalLiveSpeed(speeds: Record<number, number>): number {
  return Object.values(speeds).reduce((sum, value) => sum + (value > 0 ? value : 0), 0);
}
