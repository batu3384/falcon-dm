import { describe, expect, it } from 'vitest';
import { getDownloadCapabilities } from './downloadCapabilities';

describe('getDownloadCapabilities', () => {
  it('does not expose move for Merging but allows pause', () => {
    expect(getDownloadCapabilities('Merging')).toMatchObject({
      pause: true,
      move: false,
    });
  });

  it('allows move only for terminal completed/failed rows', () => {
    expect(getDownloadCapabilities('Completed').move).toBe(true);
    expect(getDownloadCapabilities('Failed').move).toBe(true);
    expect(getDownloadCapabilities('Downloading').move).toBe(false);
  });

  it('only exposes retry for paused and failed rows', () => {
    expect(getDownloadCapabilities('Paused').resume).toBe(true);
    expect(getDownloadCapabilities('Failed').resume).toBe(true);
    expect(getDownloadCapabilities('Queued').resume).toBe(false);
    expect(getDownloadCapabilities('Queued').pause).toBe(true);
    expect(getDownloadCapabilities('Merging').resume).toBe(false);
  });

  it('allows archive only for terminal rows', () => {
    expect(getDownloadCapabilities('Completed').archive).toBe(true);
    expect(getDownloadCapabilities('Failed').archive).toBe(true);
    expect(getDownloadCapabilities('Queued').archive).toBe(false);
    expect(getDownloadCapabilities('Downloading').archive).toBe(false);
    expect(getDownloadCapabilities('Paused').archive).toBe(false);
    expect(getDownloadCapabilities('Merging').archive).toBe(false);
  });
});
