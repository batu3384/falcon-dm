import { describe, it, expect, vi, beforeEach } from 'vitest';
import { render, screen, waitFor } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import i18n from '../i18n';
import NewDownloadSheet from './NewDownloadSheet';
import { addDownload, getSettings } from '../api/commands';
import { makeRow } from '../test/fixtures';
import type * as Commands from '../api/commands';

vi.mock('@tauri-apps/plugin-dialog', () => ({ open: vi.fn() }));
vi.mock('../api/commands', async (importOriginal) => {
  const actual = await importOriginal<typeof Commands>();
  return { ...actual, addDownload: vi.fn(), getSettings: vi.fn() };
});

describe('NewDownloadSheet', () => {
  beforeEach(() => {
    void i18n.changeLanguage('en');
    localStorage.clear();
    vi.mocked(addDownload).mockReset().mockResolvedValue(makeRow());
    vi.mocked(getSettings).mockResolvedValue({
      theme: 'system',
      default_download_path: '/Users/demo/Downloads',
      max_concurrent_downloads: 3,
      max_connections_per_server: 8,
      proxy: null,
    });
  });

  it('prefills links handed over by the clipboard monitor or a drop', () => {
    render(<NewDownloadSheet initialText="https://example.com/a.zip" onClose={vi.fn()} />);
    expect(screen.getByLabelText('Links')).toHaveValue('https://example.com/a.zip');
  });

  it('adds every link of a list and lets the engine name the files', async () => {
    const onClose = vi.fn();
    render(
      <NewDownloadSheet
        initialText={'https://example.com/a.zip\nhttps://example.com/b.zip'}
        onClose={onClose}
      />,
    );
    expect(screen.getByText('2 links')).toBeInTheDocument();
    expect(screen.queryByLabelText('File name')).toBeNull();
    await waitFor(() =>
      expect(screen.getByLabelText('Save to')).toHaveValue('/Users/demo/Downloads'),
    );
    await userEvent.click(screen.getByRole('button', { name: 'Download 2' }));
    await waitFor(() => expect(addDownload).toHaveBeenCalledTimes(2));
    expect(vi.mocked(addDownload).mock.calls[0][0]).toMatchObject({
      url: 'https://example.com/a.zip',
      filename: undefined,
      savePath: '/Users/demo/Downloads',
    });
    expect(onClose).toHaveBeenCalled();
  });

  it('offers a quality choice for YouTube and sends the selector', async () => {
    render(
      <NewDownloadSheet
        initialText="https://www.youtube.com/watch?v=dQw4w9WgXcQ"
        onClose={vi.fn()}
      />,
    );
    await userEvent.selectOptions(screen.getByLabelText('Video quality'), 'audio');
    await userEvent.click(screen.getByRole('button', { name: 'Download' }));
    await waitFor(() => expect(addDownload).toHaveBeenCalledTimes(1));
    expect(vi.mocked(addDownload).mock.calls[0][0].format).toBe('bestaudio[ext=m4a]/bestaudio');
    expect(localStorage.getItem('falcon_yt_quality')).toBe('audio');
  });

  it('shows the engine error inline for a single link', async () => {
    vi.mocked(addDownload).mockRejectedValueOnce('ERR_UNSUPPORTED_MAGNET');
    const onClose = vi.fn();
    render(<NewDownloadSheet initialText="https://example.com/a.zip" onClose={onClose} />);
    await userEvent.click(screen.getByRole('button', { name: 'Download' }));
    expect(await screen.findByRole('alert')).toHaveTextContent('Magnet and torrent links');
    expect(onClose).not.toHaveBeenCalled();
  });

  it('rejects text without a link', async () => {
    render(<NewDownloadSheet initialText="not a link" onClose={vi.fn()} />);
    expect(screen.getByText('No valid link')).toBeInTheDocument();
    await userEvent.click(screen.getByRole('button', { name: 'Download' }));
    expect(await screen.findByRole('alert')).toHaveTextContent('http:// or https://');
    expect(addDownload).not.toHaveBeenCalled();
  });
});
