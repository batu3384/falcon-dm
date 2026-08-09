import { useEffect, useState, memo } from 'react';
import { useTranslation } from 'react-i18next';
import {
  Play,
  Pause,
  ChevronUp,
  ChevronDown,
  File,
  Film,
  Music,
  Package,
  FileText,
  AppWindow,
  Folder,
  Trash2,
  AlertTriangle,
  Copy,
  Terminal,
  RotateCcw,
} from 'lucide-react';
import SpeedGraph from './SpeedGraph';
import { ConfirmDialog } from './ConfirmDialog';
import type { DownloadModel } from '../types';
import { formatBytes, calculateETA, progressPercent, fileExtension, fileFullPath } from '../types';
import { getDownloadCapabilities } from '../lib/downloadCapabilities';
import { useToastStore } from '../store/toast';
import * as api from '../api/commands';

interface DownloadItemProps {
  item: DownloadModel;
  isSelected?: boolean;
  onSelect?: (item: DownloadModel | null, mods?: { meta?: boolean; shift?: boolean }) => void;
  onRefresh?: () => void;
  isBatchSelected?: boolean;
}

function getThumb(filename: string) {
  const lower = filename.toLowerCase();
  if (lower.match(/\.(mp4|mkv|mov|avi|webm|m4v)$/)) return { icon: Film, cls: 'video' };
  if (lower.match(/\.(mp3|wav|ogg|flac|m4a|aac)$/)) return { icon: Music, cls: 'music' };
  if (lower.match(/\.(zip|rar|7z|tar|gz|bz2|xz)$/)) return { icon: Package, cls: 'archive' };
  if (lower.match(/\.(pdf|doc|docx|txt|epub|xls|xlsx|ppt|pptx)$/))
    return { icon: FileText, cls: 'document' };
  if (lower.match(/\.(dmg|pkg|exe|msi|app|deb|rpm)$/)) return { icon: AppWindow, cls: 'program' };
  return { icon: File, cls: '' };
}

function DownloadItemInner({
  item,
  isSelected,
  onSelect,
  onRefresh,
  isBatchSelected = false,
}: DownloadItemProps) {
  const { t } = useTranslation();
  const showToast = useToastStore((s) => s.showToast);
  const [menu, setMenu] = useState<{ x: number; y: number } | null>(null);
  const [confirmRemove, setConfirmRemove] = useState(false);
  const [moveOpen, setMoveOpen] = useState(false);

  const pct = progressPercent(item);
  const isDownloading = item.status === 'Downloading';
  const isCompleted = item.status === 'Completed';
  const isFailed = item.status === 'Failed';
  const capabilities = getDownloadCapabilities(item.status);
  const remainingBytes = item.total_size - item.downloaded_size;
  const etaText = isDownloading ? calculateETA(remainingBytes, item.speed) : '';
  const ext = fileExtension(item.filename);
  const { icon: ThumbIcon, cls: thumbCls } = getThumb(item.filename);

  const statusInfo: Record<string, { cls: string; label: string }> = {
    Downloading: { cls: 'downloading', label: t('downloadItem.status_downloading') },
    Merging: { cls: 'downloading', label: t('downloadItem.status_merging') },
    Completed: { cls: 'completed', label: t('downloadItem.status_completed') },
    Queued: { cls: 'queued', label: t('downloadItem.status_queued') },
    Failed: { cls: 'failed', label: t('downloadItem.status_failed') },
    Paused: { cls: 'paused', label: t('downloadItem.status_paused') },
  };
  const si = statusInfo[item.status] || { cls: 'paused', label: item.status };
  const fillCls = isCompleted ? 'done' : isFailed ? 'failed' : isDownloading ? 'active' : '';

  const call = async (fn: () => Promise<unknown>, errMsg: string) => {
    try {
      await fn();
      onRefresh?.();
    } catch {
      showToast('error', errMsg);
    }
  };

  const stop = (e: React.MouseEvent) => e.stopPropagation();

  useEffect(() => {
    if (!menu) return;
    const menuEl = document.querySelector<HTMLDivElement>('.ctx-menu');
    menuEl?.focus();
    const close = () => setMenu(null);
    const onKey = (e: KeyboardEvent) => {
      if (e.key === 'Escape') {
        e.preventDefault();
        setMenu(null);
        return;
      }
      const btns = Array.from(
        menuEl?.querySelectorAll<HTMLButtonElement>("button[role='menuitem']") ?? [],
      );
      if (!btns.length) return;
      const idx = btns.findIndex((b) => b === document.activeElement);
      if (e.key === 'ArrowDown') {
        e.preventDefault();
        btns[(idx + 1) % btns.length].focus();
      } else if (e.key === 'ArrowUp') {
        e.preventDefault();
        btns[(idx - 1 + btns.length) % btns.length].focus();
      }
    };
    window.addEventListener('click', close);
    window.addEventListener('keydown', onKey);
    return () => {
      window.removeEventListener('click', close);
      window.removeEventListener('keydown', onKey);
    };
  }, [menu]);

  const onContextMenu = (e: React.MouseEvent) => {
    e.preventDefault();
    onSelect?.(isSelected ? null : item);
    setMenu({ x: e.clientX, y: e.clientY });
  };

  const onKeyDown = (e: React.KeyboardEvent) => {
    if (e.key === 'Enter' || e.key === ' ') {
      e.preventDefault();
      onSelect?.(isSelected ? null : item);
    }
    if (e.key === 'Delete' || e.key === 'Backspace') {
      e.preventDefault();
      setConfirmRemove(true);
    }
  };

  const doRemove = () => {
    setConfirmRemove(false);
    setMenu(null);
    call(() => api.removeDownload(item.id), t('downloadItem.action_failed'));
  };

  const copyText = async (value: string, successMessage?: string) => {
    try {
      await navigator.clipboard.writeText(value);
      if (successMessage) showToast('success', successMessage);
    } catch (e) {
      showToast('error', api.extractTauriError(e));
    }
  };

  return (
    <>
      <div
        className={`dl-item ${isSelected ? 'selected' : ''} ${isBatchSelected && !isSelected ? 'batch-selected' : ''}`}
        onClick={(e) =>
          onSelect?.(isSelected && !e.metaKey && !e.shiftKey ? null : item, {
            meta: e.metaKey || e.ctrlKey,
            shift: e.shiftKey,
          })
        }
        onContextMenu={onContextMenu}
        onKeyDown={onKeyDown}
        tabIndex={0}
        role="listitem"
      >
        <div className={`dl-thumb ${thumbCls}`}>
          <ThumbIcon size={18} strokeWidth={1.6} />
        </div>

        <div className="dl-body">
          <div className="dl-top">
            <span className="dl-name" title={item.filename}>
              {item.filename}
            </span>
            {ext && <span className="badge plain">{ext}</span>}
            <span className={`badge ${si.cls}`}>{si.label}</span>
            {isFailed && item.error_message && (
              <span className="badge failed" title={item.error_message}>
                <AlertTriangle size={10} />
              </span>
            )}
          </div>

          <div className="dl-track">
            <div className={`dl-fill ${fillCls}`} style={{ width: `${pct}%` }} />
          </div>

          <div className="dl-meta">
            <span>
              {formatBytes(item.downloaded_size)} / {formatBytes(item.total_size)}
            </span>
            <span className="meta-sep">/</span>
            <span>{pct}%</span>
            {isDownloading && item.speed > 0 && (
              <>
                <span className="meta-sep">/</span>
                <span className="dl-speed-text">{formatBytes(item.speed)}/s</span>
                {etaText && (
                  <>
                    <span className="meta-sep">/</span>
                    <span>
                      {t('downloadItem.eta')} {etaText}
                    </span>
                  </>
                )}
                <SpeedGraph speed={item.speed} />
              </>
            )}
            {isCompleted && item.completed_at && (
              <>
                <span className="meta-sep">/</span>
                <span>{new Date(item.completed_at).toLocaleDateString()}</span>
              </>
            )}
          </div>
        </div>

        <div className="dl-actions">
          {capabilities.pause ? (
            <button
              type="button"
              className="icon-btn"
              onClick={(e) => {
                stop(e);
                call(() => api.pauseDownload(item.id), t('downloadItem.action_failed'));
              }}
              title={t('downloadItem.pause')}
              aria-label={t('downloadItem.pause')}
            >
              <Pause size={14} />
            </button>
          ) : capabilities.resume ? (
            <button
              type="button"
              className="icon-btn"
              onClick={(e) => {
                stop(e);
                call(() => api.resumeDownload(item.id), t('downloadItem.action_failed'));
              }}
              title={t('downloadItem.resume')}
              aria-label={t('downloadItem.resume')}
            >
              <Play size={14} />
            </button>
          ) : null}

          <button
            type="button"
            className="icon-btn"
            onClick={(e) => {
              stop(e);
              api
                .openFolder(fileFullPath(item))
                .catch(() => showToast('error', t('downloadItem.action_failed')));
            }}
            title={t('downloadItem.open_folder')}
            aria-label={t('downloadItem.open_folder')}
          >
            <Folder size={14} />
          </button>

          {item.status === 'Queued' && (
            <>
              <button
                type="button"
                className="icon-btn"
                onClick={(e) => {
                  stop(e);
                  call(() => api.changePriority(item.id, true), t('downloadItem.action_failed'));
                }}
                title={t('downloadItem.increase_priority')}
                aria-label={t('downloadItem.increase_priority')}
              >
                <ChevronUp size={14} />
              </button>
              <button
                type="button"
                className="icon-btn"
                onClick={(e) => {
                  stop(e);
                  call(() => api.changePriority(item.id, false), t('downloadItem.action_failed'));
                }}
                title={t('downloadItem.decrease_priority')}
                aria-label={t('downloadItem.decrease_priority')}
              >
                <ChevronDown size={14} />
              </button>
            </>
          )}

          {capabilities.remove && (
            <button
              type="button"
              className="icon-btn danger"
              onClick={(e) => {
                stop(e);
                setConfirmRemove(true);
              }}
              title={t('downloadItem.delete')}
              aria-label={t('downloadItem.delete')}
            >
              <Trash2 size={14} />
            </button>
          )}
        </div>
      </div>

      {menu && (
        <div className="ctx-menu" style={{ left: menu.x, top: menu.y }} role="menu" tabIndex={-1}>
          {capabilities.pause ? (
            <button
              type="button"
              role="menuitem"
              onClick={() =>
                call(() => api.pauseDownload(item.id), t('downloadItem.action_failed'))
              }
            >
              {t('downloadItem.pause')}
            </button>
          ) : capabilities.resume ? (
            <button
              type="button"
              role="menuitem"
              onClick={() =>
                call(() => api.resumeDownload(item.id), t('downloadItem.action_failed'))
              }
            >
              {t('downloadItem.resume')}
            </button>
          ) : null}
          <button
            type="button"
            role="menuitem"
            onClick={() =>
              api
                .openFolder(fileFullPath(item))
                .catch(() => showToast('error', t('downloadItem.action_failed')))
            }
          >
            {t('downloadItem.open_folder')}
          </button>
          <button type="button" role="menuitem" onClick={() => copyText(item.url)}>
            <Copy size={12} /> {t('downloadItem.copy_url')}
          </button>
          <button
            type="button"
            role="menuitem"
            onClick={() => copyText(api.buildCurlCommand(item), t('downloadItem.curl_copied'))}
          >
            <Terminal size={12} /> {t('downloadItem.copy_curl')}
          </button>
          {isCompleted && (
            <>
              <button
                type="button"
                role="menuitem"
                onClick={() =>
                  api
                    .openFile(fileFullPath(item))
                    .catch(() => showToast('error', t('downloadItem.action_failed')))
                }
              >
                {t('downloadItem.open_file')}
              </button>
            </>
          )}
          {capabilities.archive && (
            <button
              type="button"
              role="menuitem"
              onClick={() =>
                call(
                  () => api.archiveDownload(item.id, !item.archived),
                  t('downloadItem.action_failed'),
                )
              }
            >
              {item.archived ? t('downloadItem.unarchive') : t('downloadItem.archive')}
            </button>
          )}
          {capabilities.move && (
            <button type="button" role="menuitem" onClick={() => setMoveOpen(true)}>
              {t('downloadItem.move_rename')}
            </button>
          )}
          <button
            type="button"
            role="menuitem"
            onClick={() => {
              call(
                () =>
                  api.addDownload({
                    url: item.url,
                    filename: item.filename,
                    savePath: item.save_path,
                    referrer: item.referrer || undefined,
                    userAgent: item.user_agent || undefined,
                  }),
                t('downloadItem.action_failed'),
              );
            }}
          >
            <RotateCcw size={12} /> {t('downloadItem.redownload')}
          </button>
          {capabilities.remove && (
            <button
              type="button"
              role="menuitem"
              className="danger"
              onClick={() => setConfirmRemove(true)}
            >
              {t('downloadItem.delete')}
            </button>
          )}
        </div>
      )}

      {confirmRemove && (
        <ConfirmDialog
          message={t('inspector.confirm_remove')}
          onConfirm={doRemove}
          onCancel={() => setConfirmRemove(false)}
        />
      )}

      {moveOpen && (
        <MoveRenameDialog
          filename={item.filename}
          savePath={item.save_path}
          onCancel={() => setMoveOpen(false)}
          onConfirm={async (filename, savePath) => {
            try {
              await api.moveDownload(item.id, filename || undefined, savePath || undefined);
              showToast('success', t('downloadItem.moved'));
              setMoveOpen(false);
              onRefresh?.();
            } catch (e) {
              showToast('error', api.extractTauriError(e));
            }
          }}
        />
      )}
    </>
  );
}

// ponytail: memo prevents re-render of paused/completed items on every progress tick.
// Custom comparator: only re-render when item data or selection changes, ignore callback identity churn.
const DownloadItem = memo(
  DownloadItemInner,
  (prev, next) =>
    prev.item === next.item &&
    prev.isSelected === next.isSelected &&
    prev.isBatchSelected === next.isBatchSelected &&
    prev.onSelect === next.onSelect &&
    prev.onRefresh === next.onRefresh,
);

export default DownloadItem;

// ponytail: lightweight move/rename dialog. Two fields (filename + destination
// folder) + confirm. Reuses modal CSS classes; no separate file needed.
function MoveRenameDialog({
  filename,
  savePath,
  onCancel,
  onConfirm,
}: {
  filename: string;
  savePath: string;
  onCancel: () => void;
  onConfirm: (filename: string, savePath: string) => void;
}) {
  const { t } = useTranslation();
  const [fname, setFname] = useState(filename);
  const [path, setPath] = useState(savePath);
  return (
    <div className="modal-overlay" onClick={onCancel} role="presentation">
      <div
        className="modal-panel modal-sm"
        onClick={(e) => e.stopPropagation()}
        role="dialog"
        aria-modal="true"
        aria-labelledby="move-title"
      >
        <div className="modal-head">
          <h2 id="move-title" className="modal-title">
            {t('downloadItem.move_rename')}
          </h2>
        </div>
        <div className="modal-body">
          <div className="field">
            <label className="field-label" htmlFor="mv-name">
              {t('downloadItem.new_filename')}
            </label>
            <input
              id="mv-name"
              className="field-input"
              value={fname}
              onChange={(e) => setFname(e.target.value)}
              autoFocus
            />
          </div>
          <div className="field">
            <label className="field-label" htmlFor="mv-path">
              {t('downloadItem.destination_folder')}
            </label>
            <input
              id="mv-path"
              className="field-input"
              value={path}
              onChange={(e) => setPath(e.target.value)}
            />
          </div>
        </div>
        <div className="modal-foot">
          <button type="button" className="btn-secondary" onClick={onCancel}>
            {t('downloadItem.cancel')}
          </button>
          <button type="button" className="btn-primary" onClick={() => onConfirm(fname, path)}>
            {t('downloadItem.move')}
          </button>
        </div>
      </div>
    </div>
  );
}
