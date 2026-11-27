export type DownloadRowShortcut = 'open' | 'reveal' | 'menu';

type ShortcutEvent = Pick<KeyboardEvent, 'key' | 'metaKey' | 'ctrlKey' | 'shiftKey' | 'altKey'>;

/** Row shortcuts chosen not to collide with app shortcuts or macOS window keys. */
export function downloadRowShortcut(e: ShortcutEvent): DownloadRowShortcut | null {
  if (e.altKey) return null;
  const key = e.key.toLowerCase();
  const command = e.metaKey || e.ctrlKey;
  if (command && e.shiftKey && key === 'r') return 'reveal';
  if (command && !e.shiftKey && key === 'o') return 'open';
  if (!e.metaKey && e.ctrlKey && !e.shiftKey && key === ' ') return 'menu';
  return null;
}
