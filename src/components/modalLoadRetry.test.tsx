import { describe, expect, it, vi, beforeEach } from 'vitest';
import { render, screen, waitFor } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import '../i18n';
import i18n from '../i18n';
import { SchedulerModal } from './SchedulerModal';
import { SettingsModal } from './SettingsModal';
import * as api from '../api/commands';

vi.mock('../api/commands', () => ({
  getSchedule: vi.fn(),
  setSchedule: vi.fn(),
  getSettings: vi.fn(),
  getPendingPairs: vi.fn(),
  extractTauriError: (e: unknown) => (e instanceof Error ? e.message : String(e)),
}));

vi.mock('../api/events', () => ({
  onPairRequest: () => Promise.resolve(() => {}),
}));

vi.mock('@tauri-apps/plugin-dialog', () => ({
  open: vi.fn(),
}));

vi.mock('@tauri-apps/plugin-opener', () => ({
  openUrl: vi.fn(),
}));

const mockedGetSchedule = vi.mocked(api.getSchedule);
const mockedGetSettings = vi.mocked(api.getSettings);
const mockedGetPendingPairs = vi.mocked(api.getPendingPairs);

describe('modal load retry', () => {
  beforeEach(() => {
    mockedGetSchedule.mockReset();
    mockedGetSettings.mockReset();
    mockedGetPendingPairs.mockReset();
    mockedGetPendingPairs.mockResolvedValue([]);
  });

  it('scheduler load failure offers retry without hydrating Save', async () => {
    mockedGetSchedule.mockRejectedValueOnce(new Error('offline'));
    render(<SchedulerModal isOpen onClose={vi.fn()} />);
    expect(await screen.findByRole('alert')).toHaveTextContent(i18n.t('scheduler.load_failed'));
    expect(screen.getByRole('button', { name: i18n.t('scheduler.retry') })).toBeEnabled();
    expect(screen.queryByRole('button', { name: i18n.t('scheduler.save') })).not.toBeInTheDocument();

    mockedGetSchedule.mockResolvedValueOnce({
      active: false,
      start_time: null,
      stop_time: null,
    });
    await userEvent.click(screen.getByRole('button', { name: i18n.t('scheduler.retry') }));
    expect(await screen.findByRole('button', { name: i18n.t('scheduler.save') })).toBeEnabled();
  });

  it('settings load failure offers retry without hydrating Save', async () => {
    mockedGetSettings.mockRejectedValueOnce(new Error('offline'));
    render(<SettingsModal onClose={vi.fn()} />);
    expect(await screen.findByRole('alert')).toHaveTextContent('offline');
    expect(screen.getByRole('button', { name: i18n.t('settings.retry') })).toBeEnabled();
    expect(screen.queryByRole('button', { name: i18n.t('settings.save') })).not.toBeInTheDocument();

    mockedGetSettings.mockResolvedValueOnce({
      theme: 'system',
      default_download_path: '/tmp',
      max_concurrent_downloads: 3,
      max_connections_per_server: 8,
      proxy: null,
    });
    await userEvent.click(screen.getByRole('button', { name: i18n.t('settings.retry') }));
    await waitFor(() => {
      expect(screen.getByRole('button', { name: i18n.t('settings.save') })).toBeEnabled();
    });
  });
});
