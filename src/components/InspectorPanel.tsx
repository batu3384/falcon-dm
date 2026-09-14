import { useState, useEffect } from 'react';
import { useTranslation } from 'react-i18next';
import {
  X,
  ExternalLink,
  Folder,
  Copy,
  Trash2,
  Shield,
  Activity,
  HardDrive,
  Link2,
  AlertTriangle,
  RotateCcw,
} from 'lucide-react';
import type { DownloadModel } from '../types';
import { formatBytes, progressPercent, progressIndeterminate, fileFullPath, formatLocaleDate } from '../types';
import { getDownloadCapabilities } from '../lib/downloadCapabilities';
import { ConfirmDialog } from './ConfirmDialog';
import { useToastStore } from '../store/toast';
import * as api from '../api/commands';

/** Hostname-only — `evil.com/youtube.com` must not look like YouTube. */
export function youtubeDownloadHost(url: string): boolean {
  try {
    const h = new URL(url).hostname.toLowerCase();
    return (
      h === 'youtu.be' ||
      h === 'youtube.com' ||
      h.endsWith('.youtube.com') ||
      h === 'youtube-nocookie.com' ||
      h.endsWith('.youtube-nocookie.com') ||
      h === 'googlevideo.com' ||
      h.endsWith('.googlevideo.com')
    );
  } catch {
    return false;
  }
}

export function hlsDownloadUrl(url: string): boolean {
  try {
    const path = new URL(url).pathname.toLowerCase();
    return path.endsWith('.m3u8') || path.includes('.m3u8/');
  } catch {
    return false;
  }
}

interface InspectorPanelProps {
  download: DownloadModel;
  onClose: () => void;
  onRefresh?: () => void;
}

export function InspectorPanel({ download, onClose, onRefresh }: InspectorPanelProps) {
  const { t } = useTranslation();
  const showToast = useToastStore((s) => s.showToast);
  const [copied, setCopied] = useState(false);
  const [confirmRemove, setConfirmRemove] = useState(false);

  useEffect(() => {
    const handler = (e: KeyboardEvent) => {
      if (e.key === 'Escape' && !document.querySelector('.modal-overlay, .wizard-overlay')) {
        onClose();
      }
    };
    window.addEventListener('keydown', handler);
    return () => window.removeEventListener('keydown', handler);
  }, [onClose]);
  const isDownloading = download.status === 'Downloading';
  const isMerging = download.status === 'Merging';
  const isLive = isDownloading || isMerging;
  const isCompleted = download.status === 'Completed';
  const isFailed = download.status === 'Failed';
  const capabilities = getDownloadCapabilities(download.status);
  const pct = progressPercent(download);
  const indeterminate = progressIndeterminate(download);
  const startedAt = formatLocaleDate(download.created_at);
  const progressName = indeterminate
    ? t(isMerging ? 'downloadItem.status_merging' : 'downloadItem.status_downloading')
    : t('downloadItem.progress', { percent: pct });

  const handleCopyUrl = async () => {
    try {
      await navigator.clipboard.writeText(download.url);
      setCopied(true);
      setTimeout(() => setCopied(false), 2000);
    } catch (e) {
      showToast('error', api.extractTauriError(e));
    }
  };

  const handleOpenFolder = () => {
    api
      .openFolder(fileFullPath(download))
      .catch(() => showToast('error', t('inspector.folder_error')));
  };

  const handleOpenFile = () => {
    api.openFile(fileFullPath(download)).catch(() => showToast('error', t('inspector.file_error')));
  };

  const handleRemove = async (deleteFromDisk = false) => {
    try {
      await api.removeDownload(download.id, deleteFromDisk);
      setConfirmRemove(false);
      onClose();
      onRefresh?.();
    } catch (e) {
      showToast('error', api.extractTauriError(e) || t('inspector.remove_error'));
    }
  };

  const statusCls = download.status.toLowerCase();
  const statusLabel = t(`downloadItem.status_${download.status.toLowerCase()}`, download.status);
  const catKey =
    {
      Video: 'videos',
      Music: 'music',
      Document: 'documents',
      Archive: 'compressed',
      Program: 'programs',
      Other: 'other',
    }[download.category] || 'other';
  const categoryLabel = t(`sidebar.${catKey}`);

  return (
    <>
      <aside className="inspector" aria-label={t('inspector.title')}>
        <div className="inspector-head">
          <div className="inspector-head-title">
            <HardDrive size={16} aria-hidden={true} />
            <span>{t('inspector.title')}</span>
          </div>
          <button
            type="button"
            onClick={onClose}
            className="icon-btn"
            title={t('inspector.close')}
            aria-label={t('inspector.close')}
          >
            <X size={16} aria-hidden={true} />
          </button>
        </div>

        <div className="inspector-body">
          <div className="insp-card">
            <h4 className="insp-file-name" title={download.filename}>
              {download.filename}
            </h4>
            <div className="insp-file-row">
              <span className={`badge ${statusCls}`}>{statusLabel}</span>
              <span className="mono insp-file-size">{formatBytes(download.total_size)}</span>
            </div>
          </div>

          <div>
            <div className="insp-label">
              <Activity size={12} aria-hidden={true} /> {t('inspector.connections')}
            </div>
            <div className="thread-info insp-block-gap">
              <span>
                {download.aria2_gid
                  ? 'aria2'
                  : youtubeDownloadHost(download.url)
                    ? 'yt-dlp'
                    : hlsDownloadUrl(download.url)
                      ? 'HLS'
                      : 'engine'}
              </span>
              <span className="mono">
                {isLive
                  ? `${download.segments || 0} ${t('inspector.active')}`
                  : isCompleted
                    ? t('inspector.merged')
                    : t('inspector.idle')}
              </span>
            </div>
            <div
              className={`dl-track insp-block-gap${indeterminate ? ' indeterminate' : ''}`}
              role="progressbar"
              aria-label={progressName}
              aria-valuemin={0}
              aria-valuemax={100}
              aria-valuenow={indeterminate ? undefined : pct}
              aria-valuetext={indeterminate ? progressName : undefined}
            >
              <div
                className={`dl-fill ${isCompleted ? 'done' : isLive ? 'active' : ''}${indeterminate ? ' pulse' : ''}`}
                style={indeterminate ? undefined : { width: `${pct}%` }}
              />
            </div>
          </div>

          <div>
            <div className="insp-label">
              <Shield size={12} aria-hidden={true} /> {t('inspector.metrics')}
            </div>
            <div className="metric">
              <span className="metric-key">{t('inspector.downloaded')}</span>
              <span className="metric-val">
                {formatBytes(download.downloaded_size)} (
                {indeterminate ? '…' : `${pct}%`})
              </span>
            </div>
            <div className="metric">
              <span className="metric-key">{t('inspector.speed')}</span>
              <span className="metric-val">
                {isLive ? `${formatBytes(download.speed)}/s` : '-'}
              </span>
            </div>
            <div className="metric">
              <span className="metric-key">{t('inspector.category')}</span>
              <span className="metric-val">{categoryLabel}</span>
            </div>
            <div className="metric">
              <span className="metric-key">{t('inspector.priority')}</span>
              <span className="metric-val">{download.priority}</span>
            </div>
            {startedAt && (
              <div className="metric">
                <span className="metric-key">{t('inspector.started')}</span>
                <span className="metric-val">{startedAt}</span>
              </div>
            )}
          </div>

          {isFailed && download.error_message && (
            <div>
              <div className="insp-label">
                <AlertTriangle size={12} aria-hidden={true} /> {t('inspector.error_reason')}
              </div>
              <div className="insp-card insp-card-compact insp-card-danger" role="alert">
                <span className="mono insp-error-text">{download.error_message}</span>
              </div>
            </div>
          )}

          <div className="action-col">
            {capabilities.resume && (
              <button
                type="button"
                className="btn-primary"
                onClick={() =>
                  api
                    .resumeDownload(download.id)
                    .then(() => onRefresh?.())
                    .catch((e) => showToast('error', api.extractTauriError(e)))
                }
              >
                <RotateCcw size={14} aria-hidden={true} />{' '}
                {isFailed ? t('inspector.retry') : t('downloadItem.resume')}
              </button>
            )}
            {isCompleted && (
              <button type="button" className="btn-primary" onClick={handleOpenFile}>
                <ExternalLink size={14} aria-hidden={true} /> {t('inspector.open_file')}
              </button>
            )}
            <button type="button" className="btn-secondary" onClick={handleOpenFolder}>
              <Folder size={14} aria-hidden={true} /> {t('inspector.show_in_finder')}
            </button>
            <button type="button" className="btn-secondary" onClick={handleCopyUrl}>
              <Copy size={14} aria-hidden={true} />{' '}
              {copied ? t('inspector.copied') : t('inspector.copy_link')}
            </button>
            {capabilities.remove && (
              <button
                type="button"
                className="btn-ghost btn-danger-text"
                onClick={() => setConfirmRemove(true)}
              >
                <Trash2 size={14} aria-hidden={true} /> {t('inspector.remove')}
              </button>
            )}
          </div>

          <div>
            <div className="insp-label">
              <Link2 size={12} aria-hidden={true} /> {t('inspector.source_url')}
            </div>
            <div className="insp-card insp-card-compact">
              <span className="mono insp-url-text">{download.url}</span>
            </div>
          </div>
        </div>
      </aside>
      {confirmRemove && (
        <ConfirmDialog
          message={t('inspector.confirm_remove')}
          deleteFileOption
          onConfirm={handleRemove}
          onCancel={() => setConfirmRemove(false)}
        />
      )}
    </>
  );
}
