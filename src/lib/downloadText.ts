import type { DownloadModel } from '../types';
import { calculateETA, formatBytes, formatShortDate, formatSpeed, progressPercent } from './format';
import { localizeDownloadError } from '../api/commands';

type Translate = (key: string, options?: Record<string, unknown>) => string;

export interface StatusLine {
  text: string;
  tone?: 'bad' | 'warn' | 'good';
}

/** Seconds until an automatic retry, or null when none is pending. */
export function retryCountdown(row: DownloadModel, now = Date.now()): number | null {
  if (row.status !== 'Queued' || row.retry_at == null) return null;
  return Math.max(0, Math.ceil((row.retry_at - now) / 1000));
}

export function statusLabel(row: DownloadModel, t: Translate): string {
  return t(`status.${row.status.toLowerCase()}`);
}

/** Byte progress: "2,1 GB / 5,7 GB" or "120 MB" when the total is unknown. */
export function sizeProgress(row: DownloadModel, lang: string): string {
  const done = formatBytes(row.downloaded_size, lang);
  return row.total_size > 0 ? `${done} / ${formatBytes(row.total_size, lang)}` : done;
}

/** One-line summary under the filename in the list. */
export function rowStatusLine(
  row: DownloadModel,
  t: Translate,
  lang: string,
  now = Date.now(),
): StatusLine {
  const sep = ' · ';
  switch (row.status) {
    case 'Downloading': {
      const parts = [sizeProgress(row, lang)];
      if (row.speed > 0) parts.push(formatSpeed(row.speed, lang));
      const eta = calculateETA(row.total_size - row.downloaded_size, row.speed, lang);
      if (eta) parts.push(t('row.remaining', { time: eta }));
      else if (row.downloaded_size === 0) parts.unshift(t('row.connecting'));
      return { text: parts.join(sep) };
    }
    case 'Merging':
      return { text: [t('status.merging'), formatBytes(row.downloaded_size, lang)].join(sep) };
    case 'Queued': {
      const wait = retryCountdown(row, now);
      if (wait !== null) {
        return {
          text: t('row.retrying', { seconds: wait, attempt: row.attempts, max: 5 }),
          tone: 'warn',
        };
      }
      const parts = [t('status.queued')];
      if (row.downloaded_size > 0) parts.push(sizeProgress(row, lang));
      return { text: parts.join(sep) };
    }
    case 'Paused': {
      const parts = [t('status.paused')];
      if (row.downloaded_size > 0) {
        parts.push(sizeProgress(row, lang));
        if (row.total_size > 0) parts.push(t('row.percent', { value: progressPercent(row) }));
      }
      return { text: parts.join(sep) };
    }
    case 'Completed': {
      const parts = [formatBytes(row.total_size || row.downloaded_size, lang)];
      const when = formatShortDate(row.completed_at, lang);
      if (when) parts.push(when);
      return { text: parts.join(sep) };
    }
    case 'Failed':
      return {
        text: row.error_message ? localizeDownloadError(row.error_message, t) : t('status.failed'),
        tone: 'bad',
      };
  }
}
