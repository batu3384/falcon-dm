import { create } from 'zustand';
import type { DownloadModel, ProgressPayload } from '../types';
import { getDownloads } from '../api/commands';
import { downloadAdmittedToView, viewToDownloadFilter } from '../lib/downloadView';

export const DOWNLOAD_PAGE_SIZE = 200;

function liveSpeed(next: number, previous: number): number {
  return next > 0 ? next : previous;
}

function mergeLiveProgress(fresh: DownloadModel, existing?: DownloadModel): DownloadModel {
  if (!existing || existing.id !== fresh.id) return fresh;
  // Completed is sticky until the backend also reports Completed/Failed.
  // Stale Downloading ticks must not revive a finished row.
  if (
    existing.status === 'Completed' &&
    fresh.status !== 'Completed' &&
    fresh.status !== 'Failed'
  ) {
    return {
      ...fresh,
      status: 'Completed',
      downloaded_size: Math.max(fresh.downloaded_size, existing.downloaded_size),
      total_size: Math.max(fresh.total_size, existing.total_size),
      speed: 0,
    };
  }
  // Same rule as applyProgress: a poll that started before pause must not
  // revive the row. Queued is tray/IPC/schedule resume.
  if (
    existing.status === 'Paused' &&
    fresh.status !== 'Paused' &&
    fresh.status !== 'Failed' &&
    fresh.status !== 'Completed' &&
    fresh.status !== 'Queued'
  ) {
    return existing;
  }
  // HLS/ffmpeg emit Merging; a late Downloading poll or tick must not rewind UI.
  if (existing.status === 'Merging' && fresh.status === 'Downloading') {
    return {
      ...existing,
      downloaded_size: Math.max(fresh.downloaded_size, existing.downloaded_size),
      total_size: Math.max(fresh.total_size, existing.total_size),
      speed: 0,
    };
  }
  // DB terminal/paused is source of truth — do not keep a stale Downloading row.
  if (fresh.status === 'Completed' || fresh.status === 'Failed' || fresh.status === 'Paused') {
    return fresh;
  }
  const freshLive = fresh.status === 'Downloading' || fresh.status === 'Merging';
  const existingLive = existing.status === 'Downloading' || existing.status === 'Merging';
  // Scheduler/concurrency write Queued. A poll that is behind live bytes is stale.
  if (fresh.status === 'Queued') {
    if (existingLive && existing.downloaded_size > fresh.downloaded_size) {
      return {
        ...fresh,
        status: existing.status,
        downloaded_size: existing.downloaded_size,
        total_size: Math.max(fresh.total_size, existing.total_size),
        speed: liveSpeed(fresh.speed, existing.speed),
      };
    }
    return fresh;
  }
  if (existingLive && !freshLive) {
    return {
      ...fresh,
      status: existing.status,
      downloaded_size: Math.max(fresh.downloaded_size, existing.downloaded_size),
      total_size: Math.max(fresh.total_size, existing.total_size),
      speed: liveSpeed(fresh.speed, existing.speed),
    };
  }
  if (freshLive && existingLive) {
    return {
      ...fresh,
      downloaded_size: Math.max(fresh.downloaded_size, existing.downloaded_size),
      total_size: Math.max(fresh.total_size, existing.total_size),
      speed: liveSpeed(fresh.speed, existing.speed),
    };
  }
  return fresh;
}

// ponytail: downloads store absorbs the download list + selection state that
// used to live as 4 separate useStates in App.tsx and was prop-drilled into
// DownloadList, InspectorPanel, StatusBar, Sidebar. Components now subscribe
// to just the slice they need.

interface DownloadsState {
  downloads: DownloadModel[];
  loading: boolean;
  error: string | null;
  archived: boolean;
  view: string;
  search: string;
  loadedPages: number;
  hasMore: boolean;
  loadingMore: boolean;
  requestSequence: number;
  selectedDownload: DownloadModel | null;
  selectedIds: Set<number>;
  lastSelectId: number | null;

  fetchDownloads: (archived?: boolean, search?: string, view?: string) => Promise<void>;
  loadMoreDownloads: () => Promise<void>;
  retryFetch: () => Promise<void>;
  setLoading: (v: boolean) => void;
  applyProgress: (p: ProgressPayload) => void;
  addDownload: (d: DownloadModel) => void;
  selectDownload: (
    item: DownloadModel | null,
    mods?: { meta?: boolean; shift?: boolean },
    visibleIds?: number[],
  ) => void;
  clearSelection: () => void;
  syncSelectedFromList: () => void;
}

export const useDownloadsStore = create<DownloadsState>((set, get) => ({
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

  fetchDownloads: async (archived?: boolean, search?: string, view?: string) => {
    const previousArchived = get().archived;
    const previousView = get().view;
    const currentArchived = archived ?? previousArchived;
    const currentView = view ?? previousView;
    const currentSearch = search ?? get().search;
    const archiveChanged = archived !== undefined && archived !== previousArchived;
    const viewChanged = view !== undefined && view !== previousView;
    const searchChanged = search !== undefined && search !== get().search;
    const pagesToFetch =
      archiveChanged || searchChanged || viewChanged ? 1 : Math.max(1, get().loadedPages);
    const sequence = get().requestSequence + 1;
    const hasExistingRows = get().downloads.length > 0;
    set({
      archived: currentArchived,
      view: currentView,
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
      const viewFilter = viewToDownloadFilter(currentView, currentArchived);
      for (let page = 0; page < pagesToFetch; page += 1) {
        const filter =
          pagesToFetch === 1 && page === 0
            ? viewFilter
            : {
                ...viewFilter,
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
        const merged = data.map((row) =>
          mergeLiveProgress(
            row,
            state.downloads.find((download) => download.id === row.id),
          ),
        );
        const selectedDownload = state.selectedDownload
          ? (merged.find((download) => download.id === state.selectedDownload?.id) ?? null)
          : null;
        return {
          downloads: merged,
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
        ...viewToDownloadFilter(state.view, state.archived),
        ...(state.search.trim() ? { search: state.search.trim() } : {}),
        limit: DOWNLOAD_PAGE_SIZE,
        before_id: state.downloads[state.downloads.length - 1]?.id,
      });
      if (get().requestSequence !== sequence) return;
      set((current) => {
        const existingIds = new Set(current.downloads.map((download) => download.id));
        const appended = rows
          .filter((download) => !existingIds.has(download.id))
          .map((row) =>
            mergeLiveProgress(
              row,
              current.downloads.find((download) => download.id === row.id),
            ),
          );
        return {
          downloads: [...current.downloads, ...appended],
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

  retryFetch: () => get().fetchDownloads(get().archived, get().search, get().view),

  setLoading: (v) => set({ loading: v }),

  applyProgress: (p) => {
    set((state) => {
      if (!state.downloads.some((d) => d.id === p.id)) return state;
      const downloads = state.downloads.map((d) => {
        if (d.id !== p.id) return d;
        if (d.status === 'Completed') {
          if (p.status === 'Failed' || p.status === 'Paused') {
            return {
              ...d,
              status: p.status,
              speed: 0,
              downloaded_size: Math.max(p.downloaded_size, d.downloaded_size),
              total_size: Math.max(p.total_size, d.total_size),
              error_message:
                p.status === 'Failed' ? (p.error_message ?? d.error_message) : null,
            };
          }
          return {
            ...d,
            downloaded_size: Math.max(p.downloaded_size, d.downloaded_size),
            total_size: Math.max(p.total_size, d.total_size),
          };
        }
        // Stale Downloading ticks after pause must not revive the row.
        // Queued (IPC/tray resume) is the signal that the job is live again.
        if (
          d.status === 'Paused' &&
          p.status !== 'Paused' &&
          p.status !== 'Failed' &&
          p.status !== 'Completed' &&
          p.status !== 'Queued'
        ) {
          return d;
        }
        if (d.status === 'Merging' && p.status === 'Downloading') {
          return {
            ...d,
            downloaded_size: Math.max(p.downloaded_size, d.downloaded_size),
            total_size: Math.max(p.total_size, d.total_size),
            speed: 0,
          };
        }
        const halt =
          p.status === 'Completed' ||
          p.status === 'Failed' ||
          p.status === 'Paused' ||
          p.status === 'Queued' ||
          p.status === 'Merging';
        return {
          ...d,
          downloaded_size: Math.max(p.downloaded_size, d.downloaded_size),
          total_size: Math.max(p.total_size, d.total_size),
          speed: halt ? 0 : liveSpeed(p.speed, d.speed),
          status: p.status,
          segments: p.connections || d.segments,
          error_message:
            p.status === 'Failed' ? (p.error_message ?? d.error_message) : null,
        };
      });
      const selectedDownload =
        state.selectedDownload?.id === p.id
          ? (downloads.find((d) => d.id === p.id) ?? state.selectedDownload)
          : state.selectedDownload;
      return { downloads, selectedDownload };
    });
  },

  addDownload: (d) => {
    set((state) => {
      if (state.downloads.some((x) => x.id === d.id)) return state;
      if (!downloadAdmittedToView(d, state.view, state.archived)) return state;
      return { downloads: [d, ...state.downloads] };
    });
  },

  selectDownload: (item, mods, visibleIds) => {
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
          const selectedDownload =
            state.selectedDownload != null && n.has(state.selectedDownload.id)
              ? state.selectedDownload
              : (state.downloads.find((d) => n.has(d.id)) ?? null);
          return { selectedIds: n, selectedDownload };
        }
        n.add(item.id);
        return { selectedIds: n, selectedDownload: item };
      });
    } else if (mods?.shift && lastSelectId !== null) {
      const ids = visibleIds?.length ? visibleIds : downloads.map((d) => d.id);
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
