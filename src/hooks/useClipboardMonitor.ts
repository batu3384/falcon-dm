import { useEffect, useRef } from 'react';
import { readClipboard } from '../api/commands';
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

async function readClipboardText(): Promise<string> {
  const tauri = typeof window !== 'undefined' && '__TAURI_INTERNALS__' in window;
  if (tauri) return (await readClipboard()).trim();
  return (await navigator.clipboard.readText()).trim();
}

/** Poll clipboard for http(s) URLs while enabled. */
export function useClipboardMonitor(
  enabled: boolean,
  onUrl: (url: string) => void | boolean,
  onError?: (error: unknown) => void,
) {
  const lastRef = useRef('');
  const onUrlRef = useRef(onUrl);
  const onErrorRef = useRef(onError);
  const readFailed = useRef(false);
  const reading = useRef(false);
  useEffect(() => {
    onUrlRef.current = onUrl;
    onErrorRef.current = onError;
  }, [onUrl, onError]);

  useEffect(() => {
    if (!enabled) return;
    const timer = setInterval(async () => {
      if (reading.current) return;
      reading.current = true;
      try {
        const text = await readClipboardText();
        readFailed.current = false;
        if (!text || text === lastRef.current) return;
        if (!isQueueableClipboardUrl(text)) return;
        if (!clipboardUrlConsumed(onUrlRef.current(text))) return;
        lastRef.current = text;
      } catch (error) {
        if (readFailed.current) return;
        readFailed.current = true;
        onErrorRef.current?.(error);
      } finally {
        reading.current = false;
      }
    }, 2000);
    return () => clearInterval(timer);
  }, [enabled]);
}
