import * as api from '../api/commands';

const LAST_KEY = 'falcon_speed_limit_last';
// Decimal like the rest of the app: exactly 1 MB/s.
export const DEFAULT_LIMIT_KBPS = 1000;
export const MAX_SPEED_LIMIT_KBPS = 1_048_576;

/**
 * Parse the inspector's per-download cap. Empty and zero both mean "follow the
 * global limit"; only integer KB/s values can cross the Rust IPC boundary.
 */
export function parsePerDownloadSpeedLimit(input: string): number | null {
  const trimmed = input.trim();
  if (trimmed === '') return 0;
  if (!/^\d+$/.test(trimmed)) return null;
  return Math.min(Number.parseInt(trimmed, 10), MAX_SPEED_LIMIT_KBPS);
}

/** The limit the toolbar toggle restores (last non-zero value, else 1 MB/s). */
export function lastSpeedLimit(): number {
  const saved = Number(localStorage.getItem(LAST_KEY));
  return Number.isFinite(saved) && saved > 0 ? Math.floor(saved) : DEFAULT_LIMIT_KBPS;
}

export function rememberSpeedLimit(kbps: number) {
  if (kbps > 0) localStorage.setItem(LAST_KEY, String(Math.floor(kbps)));
}

/** Turn the global limit on or off. Returns the new limit in KB/s (0 = off). */
export async function toggleSpeedLimit(): Promise<number> {
  const settings = await api.getSettings();
  const current = settings.speed_limit_kbps ?? 0;
  rememberSpeedLimit(current);
  const next = current > 0 ? 0 : lastSpeedLimit();
  await api.saveSettings({ ...settings, speed_limit_kbps: next });
  return next;
}
