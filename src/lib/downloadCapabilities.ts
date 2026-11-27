import type { DownloadStatus } from '../types';

export interface DownloadCapabilities {
  pause: boolean;
  resume: boolean;
  remove: boolean;
  move: boolean;
  archive: boolean;
  open: boolean;
  priority: boolean;
  refresh: boolean;
}

const CAPABILITIES: Record<DownloadStatus, DownloadCapabilities> = {
  Queued: {
    pause: true,
    resume: false,
    remove: true,
    move: false,
    archive: false,
    open: false,
    priority: true,
    refresh: false,
  },
  Downloading: {
    pause: true,
    resume: false,
    remove: true,
    move: false,
    archive: false,
    open: false,
    priority: false,
    refresh: false,
  },
  Merging: {
    pause: true,
    resume: false,
    remove: true,
    move: false,
    archive: false,
    open: false,
    priority: false,
    refresh: false,
  },
  Paused: {
    pause: false,
    resume: true,
    remove: true,
    move: false,
    archive: false,
    open: false,
    priority: false,
    refresh: true,
  },
  Completed: {
    pause: false,
    resume: false,
    remove: true,
    move: true,
    archive: true,
    open: true,
    priority: false,
    refresh: false,
  },
  Failed: {
    pause: false,
    resume: true,
    remove: true,
    move: false,
    archive: true,
    open: false,
    priority: false,
    refresh: true,
  },
};

export function getDownloadCapabilities(status: DownloadStatus): DownloadCapabilities {
  return CAPABILITIES[status];
}

export function isLive(status: DownloadStatus): boolean {
  return status === 'Downloading' || status === 'Merging';
}
