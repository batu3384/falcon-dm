import { describe, expect, it, vi } from 'vitest';
import { buildDownloadMenu } from './downloadMenu';
import { makeRow } from '../test/fixtures';
import type { DownloadActions } from '../lib/useDownloadActions';

function stubActions(): DownloadActions {
  return {
    pause: vi.fn(),
    resume: vi.fn(),
    toggle: vi.fn(),
    remove: vi.fn(),
    reveal: vi.fn(),
    open: vi.fn(),
    copyUrl: vi.fn(),
    archive: vi.fn(),
    priority: vi.fn(),
    speedLimit: vi.fn(),
    moveRename: vi.fn(),
    redownload: vi.fn(),
    refresh: vi.fn(),
  };
}

const t = (key: string) => key;
const ids = (rows: Parameters<typeof buildDownloadMenu>[0]) =>
  buildDownloadMenu(rows, stubActions(), t).map((entry) => entry.id);

describe('buildDownloadMenu refresh entry', () => {
  it('offers link refresh for a failed row', () => {
    expect(ids([makeRow({ status: 'Failed' })])).toContain('refresh');
  });

  it('offers link refresh for a paused row', () => {
    expect(ids([makeRow({ status: 'Paused' })])).toContain('refresh');
  });

  it('hides link refresh for live, queued and completed rows', () => {
    for (const status of ['Downloading', 'Merging', 'Queued', 'Completed'] as const) {
      expect(ids([makeRow({ status })])).not.toContain('refresh');
    }
  });

  it('hides link refresh on a multi-selection', () => {
    expect(ids([makeRow({ status: 'Failed' }), makeRow({ status: 'Failed' })])).not.toContain(
      'refresh',
    );
  });

  it('runs the refresh action with the row', () => {
    const actions = stubActions();
    const row = makeRow({ status: 'Failed' });
    const entry = buildDownloadMenu([row], actions, t).find((e) => e.id === 'refresh');
    expect(entry).toBeDefined();
    if (entry && 'run' in entry) entry.run();
    expect(actions.refresh).toHaveBeenCalledWith(row);
  });
});
