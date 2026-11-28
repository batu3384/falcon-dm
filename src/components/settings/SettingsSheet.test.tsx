import { describe, it, expect, vi, beforeEach } from 'vitest';
import { fireEvent, render, screen, waitFor } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import i18n from '../../i18n';
import { openUrl } from '@tauri-apps/plugin-opener';
import { SettingsSheet } from './SettingsSheet';
import * as api from '../../api/commands';
import type * as Commands from '../../api/commands';
import { useToastStore } from '../../store/toast';

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
    checkForUpdates: vi.fn(),
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
    localStorage.clear();
    useToastStore.setState({ toasts: [] });
    vi.mocked(openUrl).mockReset().mockResolvedValue();
    vi.mocked(api.checkForUpdates).mockReset();
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

  it('enables the speed limit with the remembered value', async () => {
    localStorage.setItem('falcon_speed_limit_last', '384');
    render(<SettingsSheet initialTab="downloads" onClose={vi.fn()} />);
    await userEvent.click(await screen.findByLabelText('Limit the total download speed'));
    await userEvent.click(screen.getByRole('button', { name: 'Save' }));
    await waitFor(() => expect(api.saveSettings).toHaveBeenCalled());
    expect(vi.mocked(api.saveSettings).mock.calls[0][0].speed_limit_kbps).toBe(384);
  });

  it('keeps the current speed limit across off and on in settings', async () => {
    vi.mocked(api.getSettings).mockResolvedValue({ ...settings, speed_limit_kbps: 512 });
    render(<SettingsSheet initialTab="downloads" onClose={vi.fn()} />);
    await userEvent.click(await screen.findByLabelText('Limit the total download speed'));
    expect(localStorage.getItem('falcon_speed_limit_last')).toBe('512');
    await userEvent.click(screen.getByLabelText('Limit the total download speed'));
    await userEvent.click(screen.getByRole('button', { name: 'Save' }));
    await waitFor(() => expect(api.saveSettings).toHaveBeenCalled());
    expect(vi.mocked(api.saveSettings).mock.calls[0][0].speed_limit_kbps).toBe(512);
  });

  it('offers an update download action instead of opening the browser', async () => {
    const url = 'https://github.com/batu3384/falcon-dm/releases/tag/v0.2.0';
    vi.mocked(api.checkForUpdates).mockResolvedValue({
      current_version: '0.1.0',
      latest_version: '0.2.0',
      update_available: true,
      release_url: url,
    });
    render(<SettingsSheet initialTab="general" onClose={vi.fn()} />);
    await userEvent.click(await screen.findByRole('button', { name: 'Check for Updates' }));

    await waitFor(() => expect(useToastStore.getState().toasts).toHaveLength(1));
    expect(openUrl).not.toHaveBeenCalled();
    const action = useToastStore.getState().toasts[0].action;
    expect(action?.label).toBe('Download');
    action?.run();
    expect(openUrl).toHaveBeenCalledWith(url);
  });

  it('blocks an active schedule without two different times', async () => {
    render(<SettingsSheet initialTab="downloads" onClose={vi.fn()} />);
    await userEvent.click(await screen.findByLabelText(/Download only during these hours/));
    await userEvent.click(screen.getByRole('button', { name: 'Save' }));
    expect(await screen.findByRole('alert')).toHaveTextContent('two different times');
    expect(api.saveSettings).not.toHaveBeenCalled();
  });

  it('saves weekday schedule selections and the completion action', async () => {
    render(<SettingsSheet initialTab="downloads" onClose={vi.fn()} />);
    await userEvent.click(await screen.findByLabelText(/Download only during these hours/));
    fireEvent.change(screen.getByLabelText('Start time'), { target: { value: '23:00' } });
    fireEvent.change(screen.getByLabelText('End time'), { target: { value: '07:00' } });
    await userEvent.click(screen.getByRole('button', { name: 'Tuesday' }));
    await userEvent.selectOptions(await screen.findByLabelText('When downloads finish:'), 'script');
    await userEvent.type(await screen.findByLabelText('Script:'), '/tmp/done.sh');
    await userEvent.click(screen.getByRole('button', { name: 'Save' }));

    await waitFor(() => expect(api.saveSettings).toHaveBeenCalled());
    const saved = vi.mocked(api.saveSettings).mock.calls[0][0];
    expect(saved.schedule_days).toEqual([true, false, true, true, true, true, true]);
    expect(saved.completion_action).toBe('script');
    expect(saved.completion_script).toBe('/tmp/done.sh');
  });
});
