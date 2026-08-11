import { create } from 'zustand';
import type { DownloadModel, ProgressPayload } from '../types';
import { getDownloads } from '../api/commands';

export const DOWNLOAD_PAGE_SIZE = 200;

// ponytail: downloads store absorbs the download list + selection state that
// used to live as 4 separate useStates in App.tsx and was prop-drilled into
// DownloadList, InspectorPanel, StatusBar, Sidebar. Components now subscribe
// to just the slice they need.

interface DownloadsState {
  downloads: DownloadModel[];
  loading: boolean;
  error: string | null;
  archived: boolean;
  search: string;
  loadedPages: number;
  hasMore: boolean;
  loadingMore: boolean;
  requestSequence: number;
  selectedDownload: DownloadModel | null;
  selectedIds: Set<number>;
  lastSelectId: number | null;

  fetchDownloads: (archived?: boolean, search?: string) => Promise<void>;
  loadMoreDownloads: () => Promise<void>;
  retryFetch: () => Promise<void>;
  setLoading: (v: boolean) => void;
  applyProgress: (p: ProgressPayload) => void;
  addDownload: (d: DownloadModel) => void;
  selectDownload: (item: DownloadModel | null, mods?: { meta?: boolean; shift?: boolean }) => void;
  clearSelection: () => void;
  syncSelectedFromList: () => void;
}

export const useDownloadsStore = create<DownloadsState>((set, get) => ({
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

  fetchDownloads: async (archived?: boolean, search?: string) => {
    const previousArchived = get().archived;
    const currentArchived = archived ?? previousArchived;
    const currentSearch = search ?? get().search;
    const archiveChanged = archived !== undefined && archived !== previousArchived;
    const searchChanged = search !== undefined && search !== get().search;
    const pagesToFetch = archiveChanged || searchChanged ? 1 : Math.max(1, get().loadedPages);
    const sequence = get().requestSequence + 1;
    const hasExistingRows = get().downloads.length > 0;
    set({
      archived: currentArchived,
      search: currentSearch,
      loadedPages: pagesToFetch,
      loadingMore: false,
      loading: !hasExistingRows,
      error: null,
      requestSequence: sequence,
    });
    try {
      const pages: DownloadModel[][] = [];
      let beforeId: number | undefined;
      for (let page = 0; page < pagesToFetch; page += 1) {
        const filter =
          pagesToFetch === 1 && page === 0
            ? { archived: currentArchived }
            : {
                archived: currentArchived,
                limit: DOWNLOAD_PAGE_SIZE,
                ...(beforeId === undefined ? {} : { before_id: beforeId }),
              };
        const requestFilter = currentSearch.trim()
          ? { ...filter, search: currentSearch.trim() }
          : filter;
        const rows = await getDownloads(requestFilter);
        pages.push(rows);
        const lastRow = rows[rows.length - 1];
        beforeId = lastRow?.id;
        if (rows.length < DOWNLOAD_PAGE_SIZE) break;
      }
      const data = pages.flat();
      if (get().requestSequence !== sequence) return;
      set((state) => {
        const liveIds = new Set(data.map((download) => download.id));
        const selectedIds = new Set([...state.selectedIds].filter((id) => liveIds.has(id)));
        const selectedDownload = state.selectedDownload
          ? (data.find((download) => download.id === state.selectedDownload?.id) ?? null)
          : null;
        return {
          downloads: data,
          loading: false,
          error: null,
          hasMore: pages[pages.length - 1]?.length === DOWNLOAD_PAGE_SIZE,
          selectedIds,
          selectedDownload,
        };
      });
    } catch (e) {
      console.error('Failed to fetch downloads', e);
      if (get().requestSequence !== sequence) return;
      set({
        loading: false,
        error: e instanceof Error ? e.message : String(e),
      });
    }
  },

  loadMoreDownloads: async () => {
    const state = get();
    if (state.loadingMore || !state.hasMore) return;
    const sequence = state.requestSequence + 1;
    set({ loadingMore: true, error: null, requestSequence: sequence });
    try {
      const rows = await getDownloads({
        archived: state.archived,
        ...(state.search.trim() ? { search: state.search.trim() } : {}),
        limit: DOWNLOAD_PAGE_SIZE,
        before_id: state.downloads[state.downloads.length - 1]?.id,
      });
      if (get().requestSequence !== sequence) return;
      set((current) => {
        const existingIds = new Set(current.downloads.map((download) => download.id));
        return {
          downloads: [
            ...current.downloads,
            ...rows.filter((download) => !existingIds.has(download.id)),
          ],
          loadedPages: current.loadedPages + 1,
          hasMore: rows.length === DOWNLOAD_PAGE_SIZE,
          loadingMore: false,
        };
      });
    } catch (e) {
      if (get().requestSequence !== sequence) return;
      set({
        loadingMore: false,
        error: e instanceof Error ? e.message : String(e),
      });
    }
  },

  retryFetch: () => get().fetchDownloads(get().archived, get().search),

  setLoading: (v) => set({ loading: v }),

  applyProgress: (p) => {
    set((state) => {
      if (!state.downloads.some((d) => d.id === p.id)) return state;
      return {
        downloads: state.downloads.map((d) =>
          d.id === p.id
            ? {
                ...d,
                downloaded_size: p.downloaded_size,
                total_size: p.total_size,
                speed: p.speed,
                status: p.status,
                segments: p.connections || d.segments,
              }
            : d,
        ),
      };
    });
  },

  addDownload: (d) => {
    set((state) => {
      if (state.downloads.some((x) => x.id === d.id)) return state;
      return { downloads: [d, ...state.downloads] };
    });
  },

  selectDownload: (item, mods) => {
    if (!item) {
      set({ selectedDownload: null, selectedIds: new Set() });
      return;
    }
    const { downloads, lastSelectId } = get();
    if (mods?.meta) {
      set((state) => {
        const n = new Set(state.selectedIds);
        if (n.has(item.id)) {
          n.delete(item.id);
        } else {
          n.add(item.id);
        }
        return { selectedIds: n, selectedDownload: item };
      });
    } else if (mods?.shift && lastSelectId !== null) {
      const ids = downloads.map((d) => d.id);
      const start = ids.indexOf(lastSelectId);
      const end = ids.indexOf(item.id);
      if (start !== -1 && end !== -1) {
        const [lo, hi] = start < end ? [start, end] : [end, start];
        set({ selectedIds: new Set(ids.slice(lo, hi + 1)), selectedDownload: item });
      } else {
        set({ selectedDownload: item, selectedIds: new Set([item.id]) });
      }
    } else {
      set({ selectedDownload: item, selectedIds: new Set([item.id]) });
    }
    set({ lastSelectId: item.id });
  },

  clearSelection: () => set({ selectedIds: new Set(), selectedDownload: null }),

  // Keep the inspector selection pointing at a live row (or clear it if removed).
  syncSelectedFromList: () => {
    set((state) => {
      if (!state.selectedDownload) return state;
      const live = state.downloads.find((d) => d.id === state.selectedDownload!.id);
      if (!live) return { selectedDownload: null };
      if (live !== state.selectedDownload) return { selectedDownload: live };
      return state;
    });
  },
}));
