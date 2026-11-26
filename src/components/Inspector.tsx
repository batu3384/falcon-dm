import { useTranslation } from 'react-i18next';
import { Copy, ExternalLink, Pause, Play, RotateCw, Search, Trash2, X } from 'lucide-react';
import type { DownloadModel } from '../types';
import {
  calculateETA,
  displayPath,
  formatBytes,
  formatLocaleDate,
  formatSpeed,
  progressIndeterminate,
  progressPercent,
} from '../types';
import { getDownloadCapabilities, isLive } from '../lib/downloadCapabilities';
import { localizeDownloadError } from '../api/commands';
import { downloadEngine } from '../lib/publicHttpUrl';
import { retryCountdown, sizeProgress } from '../lib/downloadText';
import type { DownloadActions } from '../lib/useDownloadActions';
import { FileIcon } from './FileIcon';

interface InspectorProps {
  rows: DownloadModel[];
  primary: DownloadModel | null;
  actions: DownloadActions;
  onClose: () => void;
}

function cleanUrl(url: string): string {
  return url.replace(/#falconfmt=.*$/, '');
}

export function Inspector({ rows, primary, actions, onClose }: InspectorProps) {
  const { t, i18n } = useTranslation();
  const lang = i18n.language;

  if (rows.length > 1) {
    const caps = rows.map((row) => getDownloadCapabilities(row.status));
    const total = rows.reduce((sum, row) => sum + (row.total_size || row.downloaded_size), 0);
    return (
      <aside className="inspector insp-multi" aria-label={t('inspector.label')}>
        <h2 className="insp-name">{t('inspector.selected', { count: rows.length })}</h2>
        <span className="insp-status">{formatBytes(total, lang)}</span>
        <div className="insp-actions">
          {caps.some((c) => c.pause) && (
            <button type="button" className="btn" onClick={() => void actions.pause(rows)}>
              <Pause size={14} aria-hidden={true} /> {t('actions.pause')}
            </button>
          )}
          {caps.some((c) => c.resume) && (
            <button type="button" className="btn" onClick={() => void actions.resume(rows)}>
              <Play size={14} aria-hidden={true} /> {t('actions.resume')}
            </button>
          )}
          <button type="button" className="btn" onClick={() => actions.remove(rows)}>
            <Trash2 size={14} aria-hidden={true} /> {t('actions.remove')}
          </button>
        </div>
      </aside>
    );
  }

  const row = primary ?? rows[0];
  if (!row) return null;
  const caps = getDownloadCapabilities(row.status);
  const live = isLive(row.status);
  const pct = progressPercent(row);
  const indeterminate = progressIndeterminate(row);
  const eta = live ? calculateETA(row.total_size - row.downloaded_size, row.speed, lang) : '';
  const wait = retryCountdown(row);
  const failed = row.status === 'Failed';
  const statusTone = failed ? ' bad' : row.status === 'Completed' ? ' good' : '';

  return (
    <aside className="inspector" aria-label={t('inspector.label')}>
      <div className="insp-head">
        <FileIcon category={row.category} size={26} className="insp-icon" />
        <h2 className="insp-name selectable" title={row.filename}>
          {row.filename}
        </h2>
        <span className={`insp-status${statusTone}`}>
          {wait !== null
            ? t('row.retrying', { seconds: wait, attempt: row.attempts, max: 5 })
            : t(`status.${row.status.toLowerCase()}`)}
        </span>
      </div>

      {row.status !== 'Completed' && row.status !== 'Failed' && (
        <div className="insp-progress">
          <div
            className={`bar${row.status === 'Paused' || row.status === 'Queued' ? ' paused' : ''}${indeterminate ? ' indeterminate' : ''}`}
            role="progressbar"
            aria-label={t('row.progress_label', { name: row.filename })}
            aria-valuemin={0}
            aria-valuemax={100}
            aria-valuenow={indeterminate ? undefined : pct}
          >
            <span style={indeterminate ? undefined : { width: `${pct}%` }} />
          </div>
          <div className="insp-progress-line">
            <span>{sizeProgress(row, lang)}</span>
            {row.total_size > 0 && <span>{t('row.percent', { value: pct })}</span>}
          </div>
          {live && row.status === 'Downloading' && (
            <div className="insp-progress-line">
              <span>{formatSpeed(row.speed, lang)}</span>
              {eta && <span>{t('row.remaining', { time: eta })}</span>}
            </div>
          )}
        </div>
      )}

      {failed && row.error_message && (
        <div className="insp-error selectable" role="alert">
          {localizeDownloadError(row.error_message, t)}
        </div>
      )}

      <div className="insp-actions">
        {caps.open && (
          <button type="button" className="btn primary" onClick={() => void actions.open(row)}>
            <ExternalLink size={14} aria-hidden={true} /> {t('actions.open')}
          </button>
        )}
        {caps.pause && (
          <button type="button" className="btn" onClick={() => void actions.pause([row])}>
            <Pause size={14} aria-hidden={true} /> {t('actions.pause')}
          </button>
        )}
        {caps.resume && (
          <button type="button" className="btn primary" onClick={() => void actions.resume([row])}>
            {failed ? (
              <RotateCw size={14} aria-hidden={true} />
            ) : (
              <Play size={14} aria-hidden={true} />
            )}{' '}
            {failed ? t('actions.retry') : t('actions.resume')}
          </button>
        )}
        <button
          type="button"
          className="btn"
          onClick={() => void actions.reveal(row)}
          title={t('actions.reveal')}
        >
          <Search size={14} aria-hidden={true} /> {t('actions.reveal_short')}
        </button>
      </div>

      <section className="insp-section">
        <h3>{t('inspector.details')}</h3>
        <dl className="facts">
          <dt>{t('inspector.size')}</dt>
          <dd>{row.total_size > 0 ? formatBytes(row.total_size, lang) : t('inspector.unknown')}</dd>
          <dt>{t('inspector.location')}</dt>
          <dd>
            <button type="button" className="link-btn" onClick={() => void actions.reveal(row)}>
              {displayPath(row.save_path)}
            </button>
          </dd>
          <dt>{t('inspector.engine')}</dt>
          <dd>
            {downloadEngine(row.url)}
            {live && row.segments > 1
              ? ` · ${t('inspector.connections', { count: row.segments })}`
              : ''}
          </dd>
          <dt>{t('inspector.added')}</dt>
          <dd>{formatLocaleDate(row.created_at, lang)}</dd>
          {row.completed_at && (
            <>
              <dt>{t('inspector.completed')}</dt>
              <dd>{formatLocaleDate(row.completed_at, lang)}</dd>
            </>
          )}
          {row.attempts > 0 && (
            <>
              <dt>{t('inspector.attempts')}</dt>
              <dd>{row.attempts}</dd>
            </>
          )}
        </dl>
      </section>

      <section className="insp-section">
        <h3>{t('inspector.source')}</h3>
        <div className="url-box">
          <span className="selectable" title={cleanUrl(row.url)}>
            {cleanUrl(row.url)}
          </span>
          <button
            type="button"
            className="icon-btn"
            onClick={() => void actions.copyUrl(row)}
            title={t('actions.copy_link')}
            aria-label={t('actions.copy_link')}
          >
            <Copy size={13} aria-hidden={true} />
          </button>
        </div>
        {row.referrer && cleanUrl(row.referrer) !== cleanUrl(row.url) && (
          <p className="hint selectable" title={row.referrer}>
            {t('inspector.referrer')}: {row.referrer}
          </p>
        )}
      </section>

      <button
        type="button"
        className="btn plain danger insp-remove"
        onClick={() => actions.remove([row])}
      >
        <Trash2 size={14} aria-hidden={true} /> {t('actions.remove')}
      </button>

      <button
        type="button"
        className="icon-btn insp-close"
        onClick={onClose}
        title={t('inspector.hide')}
        aria-label={t('inspector.hide')}
      >
        <X size={14} aria-hidden={true} />
      </button>
    </aside>
  );
}
