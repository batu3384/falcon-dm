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

const SHA256_ABC = 'ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad';

async function expandAdvanced() {
  await userEvent.click(screen.getByRole('button', { name: 'Request options' }));
}

describe('NewDownloadSheet checksum', () => {
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

  it('sends the canonical hash for lenient input on a single link', async () => {
    render(<NewDownloadSheet initialText="https://example.com/a.zip" onClose={vi.fn()} />);
    await expandAdvanced();
    await userEvent.type(screen.getByLabelText('Checksum (optional)'), `  ${SHA256_ABC}  `);
    await userEvent.click(screen.getByRole('button', { name: 'Download' }));
    await waitFor(() => expect(addDownload).toHaveBeenCalledTimes(1));
    expect(vi.mocked(addDownload).mock.calls[0][0].expectedHash).toBe(`sha256:${SHA256_ABC}`);
  });

  it('rejects an invalid checksum inline and submits nothing', async () => {
    render(<NewDownloadSheet initialText="https://example.com/a.zip" onClose={vi.fn()} />);
    await expandAdvanced();
    await userEvent.type(screen.getByLabelText('Checksum (optional)'), 'not-a-hash');
    await userEvent.click(screen.getByRole('button', { name: 'Download' }));
    expect(await screen.findByRole('alert')).toHaveTextContent(
      'This is not a valid SHA-256 or MD5 checksum.',
    );
    expect(addDownload).not.toHaveBeenCalled();
  });

  it('omits the hash when the field is left empty', async () => {
    render(<NewDownloadSheet initialText="https://example.com/a.zip" onClose={vi.fn()} />);
    await userEvent.click(screen.getByRole('button', { name: 'Download' }));
    await waitFor(() => expect(addDownload).toHaveBeenCalledTimes(1));
    expect(vi.mocked(addDownload).mock.calls[0][0].expectedHash).toBeUndefined();
  });

  it('never offers a checksum for a list of links', async () => {
    render(
      <NewDownloadSheet
        initialText={'https://example.com/a.zip\nhttps://example.com/b.zip'}
        onClose={vi.fn()}
      />,
    );
    await expandAdvanced();
    expect(screen.queryByLabelText('Checksum (optional)')).toBeNull();
    await userEvent.click(screen.getByRole('button', { name: 'Download 2' }));
    await waitFor(() => expect(addDownload).toHaveBeenCalledTimes(2));
    expect(vi.mocked(addDownload).mock.calls[0][0].expectedHash).toBeUndefined();
  });
});
