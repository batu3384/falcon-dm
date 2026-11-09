import { describe, it, expect, vi } from 'vitest';
import { render, screen } from '@testing-library/react';
import '../i18n';
import DownloadList from './DownloadList';

describe('DownloadList empty states', () => {
  const base = {
    downloads: [],
    searchQuery: '',
    selectedId: null,
    onSelectDownload: vi.fn(),
    onRefresh: vi.fn(),
    onAddClick: vi.fn(),
  };

  it('shows category-specific copy for Video', () => {
    render(<DownloadList {...base} category="Video" />);
    expect(screen.getByRole('heading', { name: /Videos/i })).toBeInTheDocument();
    expect(screen.getByText(/Videos is empty/i)).toBeInTheDocument();
    expect(screen.getByRole('button', { name: /Add Download/i })).toBeInTheDocument();
  });

  it('shows load error when fetch fails with no rows', () => {
    render(
      <DownloadList
        {...base}
        category="All Downloads"
        error="invoke failed"
        onRetry={vi.fn()}
      />,
    );
    expect(screen.getByRole('alert')).toBeInTheDocument();
    expect(screen.getByText(/Downloads unavailable/i)).toBeInTheDocument();
  });
});
