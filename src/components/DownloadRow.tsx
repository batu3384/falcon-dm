import { memo } from 'react';
import { useTranslation } from 'react-i18next';
import { MoreHorizontal, Pause, Play, RotateCw, Search } from 'lucide-react';
import type { DownloadModel } from '../types';
import { progressIndeterminate, progressPercent } from '../types';
import { getDownloadCapabilities } from '../lib/downloadCapabilities';
import { rowStatusLine } from '../lib/downloadText';
import { FileIcon } from './FileIcon';

interface DownloadRowProps {
  row: DownloadModel;
  selected: boolean;
  /** 1-based start order while the row is queued. */
  queuePosition?: number;
  /** Re-render once a second for retry countdowns. */
  now: number;
  onToggle: (row: DownloadModel) => void;
  onReveal: (row: DownloadModel) => void;
  onMenu: (row: DownloadModel, x: number, y: number) => void;
}

function DownloadRowInner({
  row,
  selected,
  queuePosition,
  now,
  onToggle,
  onReveal,
  onMenu,
}: DownloadRowProps) {
  const { t, i18n } = useTranslation();
  const caps = getDownloadCapabilities(row.status);
  const line = rowStatusLine(row, t, i18n.language, now);
  const showBar = row.status !== 'Completed' && row.status !== 'Failed';
  const indeterminate = progressIndeterminate(row);
  const pct = progressPercent(row);
  const barClass = row.status === 'Paused' || row.status === 'Queued' ? 'bar paused' : 'bar';

  return (
    <div className={`row${selected ? ' selected' : ''}`}>
      <FileIcon category={row.category} className="row-icon" />
      <div className="row-body">
        <span className="row-name" title={row.filename}>
          {row.filename}
        </span>
        {showBar && (
          <div
            className={`${barClass}${indeterminate ? ' indeterminate' : ''}`}
            role="progressbar"
            aria-label={t('row.progress_label', { name: row.filename })}
            aria-valuemin={0}
            aria-valuemax={100}
            aria-valuenow={indeterminate ? undefined : pct}
          >
            <span style={indeterminate ? undefined : { width: `${pct}%` }} />
          </div>
        )}
        <span className="row-meta" title={line.text}>
          {row.status === 'Queued' && queuePosition !== undefined && (
            <>
              <span
                className="queue-pos"
                title={t('row.queue_position', { position: queuePosition })}
                aria-hidden="true"
              >
                #{queuePosition}
              </span>
              <span className="sr-only">
                {t('row.queue_position', { position: queuePosition })}
              </span>
            </>
          )}
          <span className={line.tone}>{line.text}</span>
        </span>
      </div>
      <div className="row-actions">
        {(caps.pause || caps.resume) && (
          <button
            type="button"
            className="row-btn"
            tabIndex={-1}
            onClick={(e) => {
              e.stopPropagation();
              onToggle(row);
            }}
            title={
              caps.pause
                ? t('actions.pause')
                : row.status === 'Failed'
                  ? t('actions.retry')
                  : t('actions.resume')
            }
            aria-label={
              caps.pause
                ? t('actions.pause')
                : row.status === 'Failed'
                  ? t('actions.retry')
                  : t('actions.resume')
            }
          >
            {caps.pause ? (
              <Pause size={14} aria-hidden={true} />
            ) : row.status === 'Failed' ? (
              <RotateCw size={14} aria-hidden={true} />
            ) : (
              <Play size={14} aria-hidden={true} />
            )}
          </button>
        )}
        <button
          type="button"
          className="row-btn"
          tabIndex={-1}
          onClick={(e) => {
            e.stopPropagation();
            onReveal(row);
          }}
          title={t('actions.reveal')}
          aria-label={t('actions.reveal')}
        >
          <Search size={14} aria-hidden={true} />
        </button>
        <button
          type="button"
          className="row-btn"
          tabIndex={-1}
          onClick={(e) => {
            e.stopPropagation();
            const rect = e.currentTarget.getBoundingClientRect();
            onMenu(row, rect.left, rect.bottom + 4);
          }}
          title={t('actions.more')}
          aria-label={t('actions.more')}
        >
          <MoreHorizontal size={15} aria-hidden={true} />
        </button>
      </div>
    </div>
  );
}

export const DownloadRow = memo(DownloadRowInner);
