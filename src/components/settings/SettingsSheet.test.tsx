import { describe, it, expect, vi, beforeEach } from 'vitest';
import { render, screen, waitFor } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import i18n from '../../i18n';
import { SettingsSheet } from './SettingsSheet';
import * as api from '../../api/commands';
import type * as Commands from '../../api/commands';

vi.mock('@tauri-apps/plugin-dialog', () => ({ open: vi.fn() }));
vi.mock('@tauri-apps/plugin-opener', () => ({ openUrl: vi.fn() }));
vi.mock('../../api/commands', async (importOriginal) => {
  const actual = await importOriginal<typeof Commands>();
  return {
    ...actual,
    getSettings: vi.fn(),
    getSchedule: vi.fn(),
    saveSettings: vi.fn(),
    setSchedule: vi.fn(),
    getExtensionStatus: vi.fn(),
    getToolStatus: vi.fn(),
  };
});

const settings = {
  theme: 'system',
  default_download_path: '/Users/demo/Downloads',
  max_concurrent_downloads: 3,
  max_connections_per_server: 8,
  proxy: null,
  speed_limit_kbps: 0,
};

describe('SettingsSheet', () => {
  beforeEach(() => {
    void i18n.changeLanguage('en');
    vi.mocked(api.getSettings).mockReset().mockResolvedValue(settings);
    vi.mocked(api.getSchedule).mockReset().mockResolvedValue({
      active: false,
      start_time: null,
      stop_time: null,
    });
    vi.mocked(api.saveSettings).mockReset().mockResolvedValue();
    vi.mocked(api.setSchedule).mockReset().mockResolvedValue();
    vi.mocked(api.getExtensionStatus).mockResolvedValue({
      approved_extension_ids: [],
      pending: [],
    });
    vi.mocked(api.getToolStatus).mockResolvedValue({
      ytdlp_path: null,
      ytdlp_version: null,
      ytdlp_error: 'ERR_YTDLP_MISSING',
      ffmpeg_path: null,
    });
  });

  it('does not offer Save until settings loaded, and retries a failed load', async () => {
    vi.mocked(api.getSettings).mockRejectedValueOnce('settings.json is corrupt');
    render(<SettingsSheet initialTab="general" onClose={vi.fn()} />);
    const retry = await screen.findByRole('button', { name: 'Try Again' });
    expect(screen.queryByRole('button', { name: 'Save' })).toBeNull();
    await userEvent.click(retry);
    expect(await screen.findByRole('button', { name: 'Save' })).toBeEnabled();
  });

  it('saves settings and the schedule together', async () => {
    const onClose = vi.fn();
    render(<SettingsSheet initialTab="downloads" onClose={onClose} />);
    const concurrent = await screen.findByLabelText('Run at the same time:');
    await userEvent.clear(concurrent);
    await userEvent.type(concurrent, '5');
    await userEvent.click(screen.getByRole('button', { name: 'Save' }));
    await waitFor(() => expect(api.saveSettings).toHaveBeenCalled());
    expect(vi.mocked(api.saveSettings).mock.calls[0][0].max_concurrent_downloads).toBe(5);
    expect(api.setSchedule).toHaveBeenCalledWith({
      startTime: null,
      stopTime: null,
      active: false,
    });
    expect(onClose).toHaveBeenCalled();
  });

  it('blocks an active schedule without two different times', async () => {
    render(<SettingsSheet initialTab="downloads" onClose={vi.fn()} />);
    await userEvent.click(await screen.findByLabelText(/Download only during these hours/));
    await userEvent.click(screen.getByRole('button', { name: 'Save' }));
    expect(await screen.findByRole('alert')).toHaveTextContent('two different times');
    expect(api.saveSettings).not.toHaveBeenCalled();
  });

  it('explains how to install a missing yt-dlp', async () => {
    render(<SettingsSheet initialTab="network" onClose={vi.fn()} />);
    expect(await screen.findByText(/yt-dlp is not installed/)).toBeInTheDocument();
    expect(screen.getByText('brew install yt-dlp')).toBeInTheDocument();
  });
});
