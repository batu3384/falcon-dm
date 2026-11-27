import { beforeEach, describe, expect, it, vi } from 'vitest';
import {
  DEFAULT_LIMIT_KBPS,
  lastSpeedLimit,
  rememberSpeedLimit,
  toggleSpeedLimit,
} from './speedLimit';
import * as api from '../api/commands';

vi.mock('../api/commands', () => ({
  getSettings: vi.fn(),
  saveSettings: vi.fn(),
}));

const baseSettings = {
  theme: 'system',
  default_download_path: '/Users/demo/Downloads',
  max_concurrent_downloads: 3,
  max_connections_per_server: 8,
  proxy: null,
  speed_limit_kbps: 0,
};

describe('speed limit memory', () => {
  beforeEach(() => {
    localStorage.clear();
    vi.mocked(api.getSettings).mockReset();
    vi.mocked(api.saveSettings).mockReset().mockResolvedValue();
  });

  it('uses the last positive value, falling back to the default', () => {
    expect(lastSpeedLimit()).toBe(DEFAULT_LIMIT_KBPS);
    rememberSpeedLimit(384.9);
    expect(lastSpeedLimit()).toBe(384);
    rememberSpeedLimit(0);
    expect(lastSpeedLimit()).toBe(384);
  });

  it('the toolbar toggle remembers the current value across off and on', async () => {
    vi.mocked(api.getSettings).mockResolvedValue({ ...baseSettings, speed_limit_kbps: 512 });

    await expect(toggleSpeedLimit()).resolves.toBe(0);
    expect(vi.mocked(api.saveSettings).mock.calls[0][0].speed_limit_kbps).toBe(0);

    vi.mocked(api.getSettings).mockResolvedValue({ ...baseSettings, speed_limit_kbps: 0 });
    await expect(toggleSpeedLimit()).resolves.toBe(512);
    expect(vi.mocked(api.saveSettings).mock.calls[1][0].speed_limit_kbps).toBe(512);
  });
});
