import { describe, expect, it } from 'vitest';
import { viewToDownloadFilter, downloadMatchesView, downloadAdmittedToView } from './downloadView';

describe('viewToDownloadFilter', () => {
  it('keeps Failed/Video on the server instead of client-slicing All', () => {
    expect(viewToDownloadFilter('Failed', false)).toEqual({ archived: false, status: 'Failed' });
    expect(viewToDownloadFilter('Video', false)).toEqual({ archived: false, category: 'Video' });
    expect(viewToDownloadFilter('Downloading', false)).toEqual({
      archived: false,
      statuses: ['Downloading', 'Merging'],
    });
    expect(viewToDownloadFilter('Paused', false)).toEqual({
      archived: false,
      statuses: ['Paused', 'Queued'],
    });
  });

  it('maps Documents/Compressed/Programs to backend categories', () => {
    expect(viewToDownloadFilter('Documents', false).category).toBe('Document');
    expect(viewToDownloadFilter('Compressed', false).category).toBe('Archive');
    expect(viewToDownloadFilter('Programs', false).category).toBe('Program');
  });

  it('uses archived-only rows for the Archived view', () => {
    expect(viewToDownloadFilter('Archived', false)).toEqual({ archived: true });
    expect(viewToDownloadFilter('All Downloads', true)).toEqual({ archived: true });
    expect(viewToDownloadFilter('All Downloads', false)).toEqual({ archived: false });
  });
});

describe('downloadMatchesView', () => {
  const queued = { status: 'Queued', category: 'Video', archived: false };

  it('rejects live rows in the Archived view', () => {
    expect(downloadMatchesView(queued, 'Archived', true)).toBe(false);
    expect(downloadMatchesView({ ...queued, archived: true }, 'Archived', true)).toBe(true);
  });

  it('rejects the wrong status/category for sidebar views', () => {
    expect(downloadMatchesView(queued, 'Failed', false)).toBe(false);
    expect(downloadMatchesView({ ...queued, status: 'Failed' }, 'Failed', false)).toBe(true);
    expect(downloadMatchesView(queued, 'Video', false)).toBe(true);
    expect(downloadMatchesView({ ...queued, category: 'Music' }, 'Video', false)).toBe(false);
  });

  it('hides Queued from the Active list until it starts', () => {
    expect(downloadMatchesView(queued, 'Downloading', false)).toBe(false);
    expect(downloadMatchesView({ ...queued, status: 'Downloading' }, 'Downloading', false)).toBe(
      true,
    );
  });
});

describe('downloadAdmittedToView', () => {
  const queued = { status: 'Queued', category: 'Video', archived: false };

  it('keeps Queued in the Active store so live progress can reveal it', () => {
    expect(downloadAdmittedToView(queued, 'Downloading', false)).toBe(true);
    expect(downloadAdmittedToView(queued, 'Failed', false)).toBe(false);
    expect(downloadAdmittedToView(queued, 'Archived', true)).toBe(false);
  });
});
