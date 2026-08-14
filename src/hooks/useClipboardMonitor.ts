import { useEffect, useRef } from 'react';

const URL_RE = /^https?:\/\/\S+/i;

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
        if (!URL_RE.test(text)) return;
        lastRef.current = text;
        onUrlRef.current(text);
      } catch {
        /* clipboard permission denied */
      }
    }, 2000);
    return () => clearInterval(timer);
  }, [enabled]);
}
