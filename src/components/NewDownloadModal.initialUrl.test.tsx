import { describe, it, expect, vi, beforeEach } from 'vitest';
import { render, screen } from '@testing-library/react';
import '../i18n';
import NewDownloadModal from './NewDownloadModal';

vi.mock('@tauri-apps/api/path', () => ({
  downloadDir: vi.fn().mockResolvedValue('/Downloads'),
}));

vi.mock('@tauri-apps/plugin-dialog', () => ({
  open: vi.fn(),
}));

vi.mock('../api/commands', () => ({
  addDownload: vi.fn(),
  getSettings: vi.fn().mockResolvedValue({ default_download_path: '/Downloads' }),
  extractTauriError: (e: unknown) => String(e),
}));

describe('NewDownloadModal initialUrl', () => {
  beforeEach(() => {
    vi.clearAllMocks();
  });

  it('prefills the URL field from clipboard monitor handoff', async () => {
    render(
      <NewDownloadModal
        onClose={vi.fn()}
        onAdded={vi.fn()}
        initialUrl="https://example.com/falcon-handoff.zip"
      />,
    );
    const field = await screen.findByDisplayValue('https://example.com/falcon-handoff.zip');
    expect(field).toBeInTheDocument();
  });
});
