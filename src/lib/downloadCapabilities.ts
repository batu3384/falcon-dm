import type { DownloadStatus } from '../types';

export interface DownloadCapabilities {
  pause: boolean;
  resume: boolean;
  remove: boolean;
  move: boolean;
  archive: boolean;
}

const CAPABILITIES: Record<DownloadStatus, DownloadCapabilities> = {
  Queued: { pause: false, resume: false, remove: true, move: false, archive: false },
  Downloading: { pause: true, resume: false, remove: true, move: false, archive: false },
  Paused: { pause: false, resume: true, remove: true, move: false, archive: false },
  Completed: { pause: false, resume: false, remove: true, move: true, archive: true },
  Failed: { pause: false, resume: true, remove: true, move: true, archive: true },
  Merging: { pause: false, resume: false, remove: true, move: false, archive: false },
};

export function getDownloadCapabilities(status: DownloadStatus): DownloadCapabilities {
  return CAPABILITIES[status];
}
