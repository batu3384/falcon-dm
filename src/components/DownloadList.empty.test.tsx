import { describe, it, expect, beforeEach } from 'vitest';
import { render, screen } from '@testing-library/react';
import '../i18n';
import i18n from '../i18n';
import DownloadList from './DownloadList';
import { useDownloadsStore } from '../store/downloads';
import { useUiStore } from '../store/ui';

describe('DownloadList empty states', () => {
  beforeEach(() => {
    void i18n.changeLanguage('en');
    useDownloadsStore.setState({
      rows: [],
      view: 'all',
      search: '',
      loading: false,
      error: null,
      hasMore: false,
      selectedIds: new Set(),
      primaryId: null,
    });
    useUiStore.setState({ newDownload: null });
  });

  it('invites the first download in the main view', () => {
    render(<DownloadList />);
    expect(screen.getByRole('heading', { name: 'No downloads yet' })).toBeInTheDocument();
    screen.getByRole('button', { name: 'New Download' }).click();
    expect(useUiStore.getState().newDownload).toBe('');
  });

  it('shows kind-specific copy without an add button', () => {
    useDownloadsStore.setState({ view: 'video' });
    render(<DownloadList />);
    expect(screen.getByRole('heading', { name: 'No videos' })).toBeInTheDocument();
    expect(screen.queryByRole('button', { name: 'New Download' })).toBeNull();
  });

  it('explains an empty search', () => {
    useDownloadsStore.setState({ search: 'zzz' });
    render(<DownloadList />);
    expect(screen.getByRole('heading', { name: 'No results for “zzz”' })).toBeInTheDocument();
  });

  it('shows a retryable error when nothing could be loaded', () => {
    useDownloadsStore.setState({ error: 'invoke failed' });
    render(<DownloadList />);
    expect(screen.getByRole('alert')).toHaveTextContent('Downloads could not be loaded');
    expect(screen.getByRole('button', { name: 'Try Again' })).toBeInTheDocument();
  });
});
