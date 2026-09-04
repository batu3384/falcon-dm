import type { DownloadStats } from '../api/commands';

// ponytail: sidebar badges must come from DB aggregates, not the filtered page.
export function libraryCountsFromStats(stats: DownloadStats): Record<string, number> {
  return {
    'All Downloads': stats.all,
    Downloading: stats.active,
    Paused: stats.paused + stats.queued,
    Failed: stats.failed,
    Completed: stats.completed,
    Archived: stats.archived,
    Video: stats.video,
    Music: stats.music,
    Documents: stats.document,
    Compressed: stats.archive,
    Programs: stats.program,
    pausable: stats.active + stats.queued,
    resumable: stats.paused + stats.failed,
  };
}
