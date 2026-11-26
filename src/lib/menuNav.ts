/** Arrow/Home/End index inside a menu. `current` may be -1. */
export function contextMenuNavIndex(len: number, current: number, key: string): number | null {
  if (len <= 0) return null;
  const idx = current < 0 || current >= len ? 0 : current;
  if (key === 'ArrowDown') return current < 0 ? 0 : (idx + 1) % len;
  if (key === 'ArrowUp') return (idx - 1 + len) % len;
  if (key === 'Home') return 0;
  if (key === 'End') return len - 1;
  return null;
}
