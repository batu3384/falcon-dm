import type { DownloadFilter } from '../api/commands';
import type { DownloadStats } from '../types';

export const LIBRARY_VIEWS = [
  'all',
  'active',
  'paused',
  'completed',
  'failed',
  'archived',
] as const;
export const CATEGORY_VIEWS = ['video', 'music', 'document', 'archive', 'program'] as const;
export type ViewId = (typeof LIBRARY_VIEWS)[number] | (typeof CATEGORY_VIEWS)[number];

const CATEGORY_OF: Record<(typeof CATEGORY_VIEWS)[number], string> = {
  video: 'Video',
  music: 'Music',
  document: 'Document',
  archive: 'Archive',
  program: 'Program',
};

type ViewRow = { status: string; category: string; archived?: boolean };

/** Sidebar view → backend filter, so a view never shows a slice of another page. */
export function viewToDownloadFilter(view: ViewId): DownloadFilter {
  if (view === 'archived') return { archived: true };
  const base: DownloadFilter = { archived: false };
  switch (view) {
    case 'active':
      return { ...base, statuses: ['Downloading', 'Merging', 'Queued'] };
    case 'paused':
      return { ...base, status: 'Paused' };
    case 'completed':
      return { ...base, status: 'Completed' };
    case 'failed':
      return { ...base, status: 'Failed' };
    case 'all':
      return base;
    default:
      return { ...base, category: CATEGORY_OF[view] };
  }
}

/** Client-side twin of `viewToDownloadFilter` for live row updates. */
export function downloadMatchesView(row: ViewRow, view: ViewId): boolean {
  if (view === 'archived') return !!row.archived;
  if (row.archived) return false;
  switch (view) {
    case 'active':
      return row.status === 'Downloading' || row.status === 'Merging' || row.status === 'Queued';
    case 'paused':
      return row.status === 'Paused';
    case 'completed':
      return row.status === 'Completed';
    case 'failed':
      return row.status === 'Failed';
    case 'all':
      return true;
    default:
      return row.category === CATEGORY_OF[view];
  }
}

/** Same contains-match as the backend `search` filter (filename or URL). */
export function downloadMatchesSearch(row: { filename: string; url: string }, search: string) {
  const q = search.trim().toLowerCase();
  return !q || row.filename.toLowerCase().includes(q) || row.url.toLowerCase().includes(q);
}

/** Sidebar badges come from DB aggregates, not from the loaded page. */
export function viewCounts(stats: DownloadStats): Record<ViewId, number> {
  return {
    all: stats.all,
    active: stats.active + stats.queued,
    paused: stats.paused,
    completed: stats.completed,
    failed: stats.failed,
    archived: stats.archived,
    video: stats.video,
    music: stats.music,
    document: stats.document,
    archive: stats.archive,
    program: stats.program,
  };
}
