import { describe, it, expect, vi, beforeEach } from 'vitest';
import { render, screen, fireEvent, waitFor } from '@testing-library/react';
import i18n from './i18n';
import App from './App';
import {
  getSettings,
  getStats,
  getExtensionStatus,
  getSchedule,
  getDownloads,
} from './api/commands';
import { ONBOARDING_KEY } from './components/OnboardingWizard';
import { useToastStore } from './store/toast';
import { useUiStore } from './store/ui';
import type * as Commands from './api/commands';

// ponytail: dropping a link-list file on the main window used to vanish
// silently — the drag overlay only armed for text/uri-list. These tests pin
// the file route: overlay shows, the New Download sheet opens prefilled, and
// unsupported/empty files surface the same toasts as the sheet's importer.

vi.mock('./api/commands', async (importOriginal) => {
  const actual = await importOriginal<typeof Commands>();
  return {
    ...actual,
    getSettings: vi.fn(),
    getDownloads: vi.fn(),
    getStats: vi.fn(),
    getExtensionStatus: vi.fn(),
    getSchedule: vi.fn(),
    setUiLanguage: vi.fn().mockResolvedValue(undefined),
    setWindowAppearance: vi.fn().mockResolvedValue(undefined),
  };
});

vi.mock('./api/events', () => ({
  onDownloadAdded: () => Promise.resolve(() => {}),
  onDownloadUpdated: () => Promise.resolve(() => {}),
  onDownloadRemoved: () => Promise.resolve(() => {}),
  onDownloadProgress: () => Promise.resolve(() => {}),
  onOpenNewDownload: () => Promise.resolve(() => {}),
  onPairRequest: () => Promise.resolve(() => {}),
}));

function dropDataTransfer(files: File[]) {
  return {
    files,
    types: ['Files'],
    getData: () => '',
  };
}

describe('App file drop', () => {
  beforeEach(() => {
    void i18n.changeLanguage('en');
    localStorage.clear();
    localStorage.setItem(ONBOARDING_KEY, 'true');
    useToastStore.setState({ toasts: [] });
    useUiStore.setState({ newDownload: null, settingsTab: null, logsOpen: false });
    vi.mocked(getSettings).mockResolvedValue({
      theme: 'system',
      default_download_path: '/Users/demo/Downloads',
      max_concurrent_downloads: 3,
      max_connections_per_server: 8,
      proxy: null,
    });
    vi.mocked(getDownloads).mockResolvedValue([]);
    vi.mocked(getStats).mockResolvedValue({
      active: 0,
      queued: 0,
      paused: 0,
      completed: 0,
      failed: 0,
      total_downloaded_bytes: 0,
      current_speed: 0,
      all: 0,
      archived: 0,
      video: 0,
      music: 0,
      document: 0,
      archive: 0,
      program: 0,
      free_disk_bytes: null,
      speed_limit_kbps: 0,
    });
    vi.mocked(getExtensionStatus).mockResolvedValue({
      approved_extension_ids: [],
      pending: [],
    });
    vi.mocked(getSchedule).mockResolvedValue({ start_time: null, stop_time: null, active: false });
  });

  it('shows the drop overlay when files are dragged over the window', () => {
    const { container } = render(<App />);
    const app = container.querySelector('.app') as HTMLElement;
    fireEvent.dragOver(app, { dataTransfer: { types: ['Files'] } });
    expect(screen.getByText('Drop links to download')).toBeInTheDocument();
  });

  it('opens the New Download sheet prefilled with the links of a dropped .txt', async () => {
    const { container } = render(<App />);
    const app = container.querySelector('.app') as HTMLElement;
    const file = new File(['https://example.com/a.zip\nhttps://example.com/b.zip'], 'links.txt', {
      type: 'text/plain',
    });
    fireEvent.drop(app, { dataTransfer: dropDataTransfer([file]) });
    const textarea = (await screen.findByLabelText('Links')) as HTMLTextAreaElement;
    expect(textarea.value).toBe('https://example.com/a.zip\nhttps://example.com/b.zip');
  });

  it('rejects an unsupported file type with a toast and no sheet', async () => {
    const { container } = render(<App />);
    const app = container.querySelector('.app') as HTMLElement;
    const file = new File(['https://example.com/a.zip'], 'notes.pdf', {
      type: 'application/pdf',
    });
    fireEvent.drop(app, { dataTransfer: dropDataTransfer([file]) });
    await waitFor(() =>
      expect(useToastStore.getState().toasts.map((t) => t.msg)).toContain(
        'Only .txt and .csv files can be imported.',
      ),
    );
    expect(screen.queryByLabelText('Links')).toBeNull();
  });

  it('reports a link-less text file instead of opening an empty sheet', async () => {
    const { container } = render(<App />);
    const app = container.querySelector('.app') as HTMLElement;
    const file = new File(['no links here'], 'links.txt', { type: 'text/plain' });
    fireEvent.drop(app, { dataTransfer: dropDataTransfer([file]) });
    await waitFor(() =>
      expect(useToastStore.getState().toasts.map((t) => t.msg)).toContain(
        'The file contains no links.',
      ),
    );
    expect(screen.queryByLabelText('Links')).toBeNull();
  });
});
