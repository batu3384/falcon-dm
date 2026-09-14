/** Toolbar Gauge vs Settings `speed_limit_kbps` — keep the last non-zero as restore. */
export function speedLimitFromSettings(
  kbps: number | undefined | null,
  previousSaved: number,
): { limited: boolean; saved: number } {
  const n = typeof kbps === 'number' && Number.isFinite(kbps) && kbps > 0 ? Math.floor(kbps) : 0;
  return { limited: n > 0, saved: n > 0 ? n : previousSaved };
}
