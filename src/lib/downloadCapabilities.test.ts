import { describe, expect, it } from 'vitest';
import { getDownloadCapabilities, isLive } from './downloadCapabilities';

describe('getDownloadCapabilities', () => {
  it('pauses anything that is running or waiting', () => {
    for (const status of ['Queued', 'Downloading', 'Merging'] as const) {
      expect(getDownloadCapabilities(status).pause).toBe(true);
      expect(getDownloadCapabilities(status).resume).toBe(false);
    }
  });

  it('resumes paused rows and retries failed ones', () => {
    expect(getDownloadCapabilities('Paused').resume).toBe(true);
    expect(getDownloadCapabilities('Failed').resume).toBe(true);
    expect(getDownloadCapabilities('Completed').resume).toBe(false);
  });

  it('opens and moves only completed files, like the backend', () => {
    expect(getDownloadCapabilities('Completed')).toMatchObject({ open: true, move: true });
    expect(getDownloadCapabilities('Failed')).toMatchObject({ open: false, move: false });
    expect(getDownloadCapabilities('Downloading').move).toBe(false);
  });

  it('archives only finished rows and reorders only queued ones', () => {
    expect(getDownloadCapabilities('Completed').archive).toBe(true);
    expect(getDownloadCapabilities('Failed').archive).toBe(true);
    expect(getDownloadCapabilities('Paused').archive).toBe(false);
    expect(getDownloadCapabilities('Queued').priority).toBe(true);
    expect(getDownloadCapabilities('Downloading').priority).toBe(false);
  });

  it('refreshes the link of stopped rows only, like the backend', () => {
    expect(getDownloadCapabilities('Failed').refresh).toBe(true);
    expect(getDownloadCapabilities('Paused').refresh).toBe(true);
    for (const status of ['Queued', 'Downloading', 'Merging', 'Completed'] as const) {
      expect(getDownloadCapabilities(status).refresh).toBe(false);
    }
  });

  it('treats Downloading and Merging as live', () => {
    expect(isLive('Downloading')).toBe(true);
    expect(isLive('Merging')).toBe(true);
    expect(isLive('Queued')).toBe(false);
  });
});
