import type { DownloadFilter } from '../api/commands';

type ViewRow = { status: string; category: string; archived?: boolean };

/** Map sidebar view → backend filter so Failed/Video/etc. are not sliced off the unfiltered page. */
export function viewToDownloadFilter(view: string, archived: boolean): DownloadFilter {
  if (archived || view === 'Archived') return { archived: true };
  const base: DownloadFilter = { archived: false };
  switch (view) {
    case 'Downloading':
      return { ...base, statuses: ['Downloading', 'Merging'] };
    case 'Paused':
      return { ...base, statuses: ['Paused', 'Queued'] };
    case 'Failed':
      return { ...base, status: 'Failed' };
    case 'Completed':
      return { ...base, status: 'Completed' };
    case 'Video':
      return { ...base, category: 'Video' };
    case 'Music':
      return { ...base, category: 'Music' };
    case 'Documents':
      return { ...base, category: 'Document' };
    case 'Compressed':
      return { ...base, category: 'Archive' };
    case 'Programs':
      return { ...base, category: 'Program' };
    default:
      return base;
  }
}

/** Same rules as the sidebar fetch — list UI must not show the wrong rows. */
export function downloadMatchesView(row: ViewRow, view: string, archived: boolean): boolean {
  if (archived || view === 'Archived') return !!row.archived;
  if (row.archived) return false;
  switch (view) {
    case 'Downloading':
      return row.status === 'Downloading' || row.status === 'Merging';
    case 'Paused':
      return row.status === 'Paused' || row.status === 'Queued';
    case 'Failed':
      return row.status === 'Failed';
    case 'Completed':
      return row.status === 'Completed';
    case 'Video':
      return row.category === 'Video';
    case 'Music':
      return row.category === 'Music';
    case 'Documents':
      return row.category === 'Document';
    case 'Compressed':
      return row.category === 'Archive';
    case 'Programs':
      return row.category === 'Program';
    default:
      return true;
  }
}

/**
 * Live `download-added` may enter the store only when this view can show it
 * now, or (Active) after Queued → Downloading without a refetch.
 */
export function downloadAdmittedToView(row: ViewRow, view: string, archived: boolean): boolean {
  if (archived || view === 'Archived') return !!row.archived;
  if (row.archived) return false;
  if (view === 'Downloading') {
    return row.status === 'Downloading' || row.status === 'Merging' || row.status === 'Queued';
  }
  return downloadMatchesView(row, view, archived);
}
