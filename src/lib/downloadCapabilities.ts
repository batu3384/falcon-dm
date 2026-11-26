import type { DownloadStatus } from '../types';

export interface DownloadCapabilities {
  pause: boolean;
  resume: boolean;
  remove: boolean;
  move: boolean;
  archive: boolean;
  open: boolean;
  priority: boolean;
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
  },
  Downloading: {
    pause: true,
    resume: false,
    remove: true,
    move: false,
    archive: false,
    open: false,
    priority: false,
  },
  Merging: {
    pause: true,
    resume: false,
    remove: true,
    move: false,
    archive: false,
    open: false,
    priority: false,
  },
  Paused: {
    pause: false,
    resume: true,
    remove: true,
    move: false,
    archive: false,
    open: false,
    priority: false,
  },
  Completed: {
    pause: false,
    resume: false,
    remove: true,
    move: true,
    archive: true,
    open: true,
    priority: false,
  },
  Failed: {
    pause: false,
    resume: true,
    remove: true,
    move: false,
    archive: true,
    open: false,
    priority: false,
  },
};

export function getDownloadCapabilities(status: DownloadStatus): DownloadCapabilities {
  return CAPABILITIES[status];
}

export function isLive(status: DownloadStatus): boolean {
  return status === 'Downloading' || status === 'Merging';
}
