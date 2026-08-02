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
import { formatBytes, progressPercent, fileFullPath } from '../types';
import { ConfirmDialog } from './ConfirmDialog';
import { useToastStore } from '../store/toast';
import * as api from '../api/commands';

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
      if (e.key === 'Escape' && !document.querySelector('.modal-overlay')) onClose();
    };
    window.addEventListener('keydown', handler);
    return () => window.removeEventListener('keydown', handler);
  }, [onClose]);
  const isDownloading = download.status === 'Downloading';
  const isCompleted = download.status === 'Completed';
  const isFailed = download.status === 'Failed';
  const pct = progressPercent(download);

  const handleCopyUrl = () => {
    navigator.clipboard.writeText(download.url).then(() => {
      setCopied(true);
      setTimeout(() => setCopied(false), 2000);
    });
  };

  const handleOpenFolder = () => {
    api
      .openFolder(fileFullPath(download))
      .catch(() => showToast('error', t('inspector.folder_error')));
  };

  const handleOpenFile = () => {
    api.openFile(fileFullPath(download)).catch(() => showToast('error', t('inspector.file_error')));
  };

  const handleRemove = async () => {
    try {
      await api.removeDownload(download.id);
      setConfirmRemove(false);
      onClose();
      onRefresh?.();
    } catch {
      showToast('error', t('inspector.remove_error'));
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
      <aside className="inspector">
        <div className="inspector-head">
          <div className="inspector-head-title">
            <HardDrive size={16} style={{ color: 'var(--text-3)' }} />
            <span>{t('inspector.title')}</span>
          </div>
          <button
            type="button"
            onClick={onClose}
            className="icon-btn"
            title={t('inspector.close')}
            aria-label={t('inspector.close')}
          >
            <X size={16} />
          </button>
        </div>

        <div className="inspector-body">
          <div className="insp-card">
            <h4 className="insp-file-name" title={download.filename}>
              {download.filename}
            </h4>
            <div className="insp-file-row">
              <span className={`badge ${statusCls}`}>{statusLabel}</span>
              <span className="mono" style={{ fontSize: 12, color: 'var(--text-2)' }}>
                {formatBytes(download.total_size)}
              </span>
            </div>
          </div>

          <div>
            <div className="insp-label">
              <Activity size={12} /> {t('inspector.connections')}
            </div>
            <div className="thread-info" style={{ marginTop: 8 }}>
              <span>
                {download.aria2_gid
                  ? 'aria2'
                  : download.url.includes('youtube') || download.url.includes('youtu.be')
                    ? 'yt-dlp'
                    : download.url.includes('.m3u8')
                      ? 'HLS'
                      : 'engine'}
              </span>
              <span className="mono">
                {isDownloading
                  ? `${download.segments || 0} ${t('inspector.active')}`
                  : isCompleted
                    ? t('inspector.merged')
                    : t('inspector.idle')}
              </span>
            </div>
            <div className="dl-track" style={{ marginTop: 8 }}>
              <div
                className={`dl-fill ${isCompleted ? 'done' : isDownloading ? 'active' : ''}`}
                style={{ width: `${pct}%` }}
              />
            </div>
          </div>

          <div>
            <div className="insp-label">
              <Shield size={12} /> {t('inspector.metrics')}
            </div>
            <div className="metric">
              <span className="metric-key">{t('inspector.downloaded')}</span>
              <span className="metric-val">
                {formatBytes(download.downloaded_size)} ({pct}%)
              </span>
            </div>
            <div className="metric">
              <span className="metric-key">{t('inspector.speed')}</span>
              <span className="metric-val">
                {isDownloading ? `${formatBytes(download.speed)}/s` : '-'}
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
            {download.created_at && (
              <div className="metric">
                <span className="metric-key">{t('inspector.started')}</span>
                <span className="metric-val">{new Date(download.created_at).toLocaleString()}</span>
              </div>
            )}
          </div>

          {isFailed && download.error_message && (
            <div>
              <div className="insp-label">
                <AlertTriangle size={12} /> {t('inspector.error_reason')}
              </div>
              <div
                className="insp-card"
                style={{ padding: '10px 12px', borderColor: 'var(--danger-soft)' }}
              >
                <span
                  className="mono"
                  style={{
                    fontSize: 11,
                    wordBreak: 'break-all',
                    color: 'var(--danger)',
                    lineHeight: 1.5,
                  }}
                >
                  {download.error_message}
                </span>
              </div>
            </div>
          )}

          <div className="action-col">
            {isFailed && (
              <button
                className="btn-primary"
                onClick={() =>
                  api
                    .resumeDownload(download.id)
                    .then(() => onRefresh?.())
                    .catch(() => showToast('error', t('inspector.remove_error')))
                }
              >
                <RotateCcw size={14} /> {t('inspector.retry')}
              </button>
            )}
            {isCompleted && (
              <button className="btn-primary" onClick={handleOpenFile}>
                <ExternalLink size={14} /> {t('inspector.open_file')}
              </button>
            )}
            <button className="btn-secondary" onClick={handleOpenFolder}>
              <Folder size={14} /> {t('inspector.show_in_finder')}
            </button>
            <button className="btn-secondary" onClick={handleCopyUrl}>
              <Copy size={14} /> {copied ? t('inspector.copied') : t('inspector.copy_link')}
            </button>
            <button
              className="btn-ghost"
              onClick={() => setConfirmRemove(true)}
              style={{ color: 'var(--danger)' }}
            >
              <Trash2 size={14} /> {t('inspector.remove')}
            </button>
          </div>

          <div>
            <div className="insp-label">
              <Link2 size={12} /> {t('inspector.source_url')}
            </div>
            <div className="insp-card" style={{ padding: '10px 12px' }}>
              <span
                className="mono"
                style={{
                  fontSize: 11,
                  wordBreak: 'break-all',
                  color: 'var(--text-2)',
                  lineHeight: 1.5,
                }}
              >
                {download.url}
              </span>
            </div>
          </div>
        </div>
      </aside>
      {confirmRemove && (
        <ConfirmDialog
          message={t('inspector.confirm_remove')}
          onConfirm={handleRemove}
          onCancel={() => setConfirmRemove(false)}
        />
      )}
    </>
  );
}
