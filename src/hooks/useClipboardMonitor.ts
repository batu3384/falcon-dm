import { useEffect, useRef } from 'react';
import { isPublicHttpUrl } from '../lib/publicHttpUrl';

export const URL_RE = /^https?:\/\/\S+/i;
const GLOBAL_JUNK_PATH_RE = /no_input\.mp3|\/generate_204|favicon|\.svg(\?|$)/i;
const YOUTUBE_UI_PATH_RE = /\/api\/stats|\/log_event|\/ptracking|\/s\/search\//i;

function isYoutubeClipboardHost(host: string): boolean {
  return (
    host === 'youtu.be' ||
    host === 'youtube.com' ||
    host.endsWith('.youtube.com') ||
    host === 'youtube-nocookie.com' ||
    host.endsWith('.youtube-nocookie.com')
  );
}

export function isQueueableClipboardUrl(text: string): boolean {
  const value = text.trim();
  if (value.toLowerCase().startsWith('magnet:')) return false;
  if (!isPublicHttpUrl(value)) return false;
  try {
    const parsed = new URL(value);
    const host = parsed.hostname.toLowerCase();
    const pathQ = `${parsed.pathname}${parsed.search}`;
    if (GLOBAL_JUNK_PATH_RE.test(pathQ)) return false;
    if (host === 'doubleclick.net' || host.endsWith('.doubleclick.net')) return false;
    if (isYoutubeClipboardHost(host) && YOUTUBE_UI_PATH_RE.test(pathQ)) return false;
    return true;
  } catch {
    return false;
  }
}

/** `false` from onUrl means overlay blocked — keep polling the same URL. */
export function clipboardUrlConsumed(accepted: unknown): boolean {
  return accepted !== false;
}

/** Poll clipboard for http(s) URLs while enabled. */
export function useClipboardMonitor(enabled: boolean, onUrl: (url: string) => void | boolean) {
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
        if (!clipboardUrlConsumed(onUrlRef.current(text))) return;
        lastRef.current = text;
      } catch {
        /* clipboard permission denied */
      }
    }, 2000);
    return () => clearInterval(timer);
  }, [enabled]);
}
