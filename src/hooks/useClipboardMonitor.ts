import { useEffect, useRef } from 'react';

export const URL_RE = /^https?:\/\/\S+/i;
const JUNK_CLIPBOARD_RE =
  /no_input\.mp3|\/generate_204|\/api\/stats|\/log_event|favicon|\.svg(\?|$)/i;

export function isQueueableClipboardUrl(text: string): boolean {
  const value = text.trim();
  if (value.toLowerCase().startsWith('magnet:')) return false;
  if (!URL_RE.test(value) || value.length > 2048) return false;
  if (JUNK_CLIPBOARD_RE.test(value)) return false;
  return true;
}

/** Poll clipboard for http(s) URLs while enabled. */
export function useClipboardMonitor(enabled: boolean, onUrl: (url: string) => void) {
  const lastRef = useRef('');
  const onUrlRef = useRef(onUrl);
  useEffect(() => {
    onUrlRef.current = onUrl;
  }, [onUrl]);

  useEffect(() => {
    if (!enabled) return;
    const timer = setInterval(async () => {
      try {
        const text = (await navigator.clipboard.readText()).trim();
        if (!text || text === lastRef.current) return;
        if (!isQueueableClipboardUrl(text)) return;
        lastRef.current = text;
        onUrlRef.current(text);
      } catch {
        /* clipboard permission denied */
      }
    }, 2000);
    return () => clearInterval(timer);
  }, [enabled]);
}
