import { useEffect, useRef, useState } from 'react';
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
import { checksumAlgoLabel } from '../lib/checksum';
import { MAX_SPEED_LIMIT_KBPS, parsePerDownloadSpeedLimit } from '../lib/speedLimit';
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

function speedLimitText(kbps: number): string {
  return kbps > 0 ? String(kbps) : '';
}

function SpeedLimitField({ row, onSave }: { row: DownloadModel; onSave: (kbps: number) => void }) {
  const { t } = useTranslation();
  const current = row.speed_limit_kbps ?? 0;
  const [text, setText] = useState(() => speedLimitText(current));
  const committed = useRef(current);

  // Progress events replace the row object constantly; resync only when the
  // persisted cap itself (or the inspected row) changes, never while typing.
  useEffect(() => {
    committed.current = current;
    setText(speedLimitText(current));
  }, [row.id, current]);

  const reset = () => setText(speedLimitText(committed.current));
  const commit = () => {
    const parsed = parsePerDownloadSpeedLimit(text);
    if (parsed === null) {
      reset();
      return;
    }
    setText(speedLimitText(parsed));
    if (parsed !== committed.current) {
      committed.current = parsed;
      onSave(parsed);
    }
  };

  return (
    <>
      <div className="stepper">
        <input
          className="input narrow"
          type="number"
          inputMode="numeric"
          min={0}
          max={MAX_SPEED_LIMIT_KBPS}
          step={1}
          value={text}
          placeholder={t('inspector.speed_limit_global')}
          aria-label={t('inspector.speed_limit')}
          spellCheck={false}
          onChange={(event) => setText(event.target.value)}
          onBlur={commit}
          onKeyDown={(event) => {
            if (event.key === 'Enter') {
              event.preventDefault();
              commit();
              event.currentTarget.blur();
            } else if (event.key === 'Escape') {
              event.preventDefault();
              reset();
            }
          }}
        />
        <span className="unit">KB/s</span>
      </div>
      <p className="hint">{t('inspector.speed_limit_hint')}</p>
    </>
  );
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
          <dt>{t('inspector.speed_limit')}</dt>
          <dd>
            <SpeedLimitField row={row} onSave={(kbps) => actions.speedLimit(row, kbps)} />
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
          {row.expected_hash && (
            <>
              <dt>{t('inspector.checksum')}</dt>
              {row.status === 'Completed' ? (
                // Verification runs before the file lands, so a completed row
                // with a hash is a verified file.
                <dd className="verified">
                  {t('inspector.checksum_verified', {
                    algo: checksumAlgoLabel(row.expected_hash),
                  })}
                </dd>
              ) : row.status === 'Failed' && row.error_message === 'ERR_CHECKSUM_MISMATCH' ? (
                <dd className="mismatch">
                  {t('inspector.checksum_mismatch', {
                    algo: checksumAlgoLabel(row.expected_hash),
                  })}
                </dd>
              ) : (
                <dd>{checksumAlgoLabel(row.expected_hash)}</dd>
              )}
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
