import { invoke } from "@tauri-apps/api/core";
import { useEffect, useState } from "react";
import { useTranslation } from "react-i18next";
import {
  Play, Pause, ChevronUp, ChevronDown, File, Film, Music, Package,
  FileText, AppWindow, Folder, Trash2, AlertTriangle, Copy,
} from "lucide-react";
import SpeedGraph from "./SpeedGraph";
import { ConfirmDialog } from "./ConfirmDialog";
import type { DownloadModel } from "../types";
import { formatBytes, calculateETA, progressPercent, fileExtension, fileFullPath } from "../types";

type ToastFn = (kind: "success" | "error" | "info", msg: string) => void;

interface DownloadItemProps {
  item: DownloadModel;
  isSelected?: boolean;
  onSelect?: () => void;
  onRefresh?: () => void;
  showToast: ToastFn;
}

function getThumb(filename: string) {
  const lower = filename.toLowerCase();
  if (lower.match(/\.(mp4|mkv|mov|avi|webm|m4v)$/)) return { icon: Film, cls: "video" };
  if (lower.match(/\.(mp3|wav|ogg|flac|m4a|aac)$/)) return { icon: Music, cls: "music" };
  if (lower.match(/\.(zip|rar|7z|tar|gz|bz2|xz)$/)) return { icon: Package, cls: "archive" };
  if (lower.match(/\.(pdf|doc|docx|txt|epub|xls|xlsx|ppt|pptx)$/)) return { icon: FileText, cls: "document" };
  if (lower.match(/\.(dmg|pkg|exe|msi|app|deb|rpm)$/)) return { icon: AppWindow, cls: "program" };
  return { icon: File, cls: "" };
}

export default function DownloadItem({ item, isSelected, onSelect, onRefresh, showToast }: DownloadItemProps) {
  const { t } = useTranslation();
  const [menu, setMenu] = useState<{ x: number; y: number } | null>(null);
  const [confirmRemove, setConfirmRemove] = useState(false);

  const pct = progressPercent(item);
  const isDownloading = item.status === "Downloading";
  const isCompleted = item.status === "Completed";
  const isFailed = item.status === "Failed";
  const remainingBytes = item.total_size - item.downloaded_size;
  const etaText = isDownloading ? calculateETA(remainingBytes, item.speed) : "";
  const ext = fileExtension(item.filename);
  const { icon: ThumbIcon, cls: thumbCls } = getThumb(item.filename);

  const statusInfo: Record<string, { cls: string; label: string }> = {
    Downloading: { cls: "downloading", label: t("downloadItem.status_downloading") },
    Merging: { cls: "downloading", label: t("downloadItem.status_merging") },
    Completed: { cls: "completed", label: t("downloadItem.status_completed") },
    Queued: { cls: "queued", label: t("downloadItem.status_queued") },
    Failed: { cls: "failed", label: t("downloadItem.status_failed") },
    Paused: { cls: "paused", label: t("downloadItem.status_paused") },
  };
  const si = statusInfo[item.status] || { cls: "paused", label: item.status };
  const fillCls = isCompleted ? "done" : isFailed ? "failed" : isDownloading ? "active" : "";

  const call = async (fn: () => Promise<unknown>, errMsg: string) => {
    try {
      await fn();
      onRefresh?.();
    } catch {
      showToast("error", errMsg);
    }
  };

  const stop = (e: React.MouseEvent) => e.stopPropagation();

  useEffect(() => {
    if (!menu) return;
    const close = () => setMenu(null);
    window.addEventListener("click", close);
    return () => window.removeEventListener("click", close);
  }, [menu]);

  const onContextMenu = (e: React.MouseEvent) => {
    e.preventDefault();
    onSelect?.();
    setMenu({ x: e.clientX, y: e.clientY });
  };

  const onKeyDown = (e: React.KeyboardEvent) => {
    if (e.key === "Enter" || e.key === " ") {
      e.preventDefault();
      onSelect?.();
    }
    if (e.key === "Delete" || e.key === "Backspace") {
      e.preventDefault();
      setConfirmRemove(true);
    }
  };

  const doRemove = () => {
    setConfirmRemove(false);
    setMenu(null);
    call(() => invoke("remove_download", { id: item.id }), t("downloadItem.action_failed"));
  };

  return (
    <>
      <div
        className={`dl-item ${isSelected ? "selected" : ""}`}
        onClick={onSelect}
        onContextMenu={onContextMenu}
        onKeyDown={onKeyDown}
        role="row"
        tabIndex={0}
        aria-selected={isSelected}
      >
        <div className={`dl-thumb ${thumbCls}`}>
          <ThumbIcon size={18} strokeWidth={1.6} />
        </div>

        <div className="dl-body">
          <div className="dl-top">
            <span className="dl-name" title={item.filename}>{item.filename}</span>
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
            <span>{formatBytes(item.downloaded_size)} / {formatBytes(item.total_size)}</span>
            <span className="meta-sep">/</span>
            <span>{pct}%</span>
            {isDownloading && item.speed > 0 && (
              <>
                <span className="meta-sep">/</span>
                <span className="dl-speed-text">{formatBytes(item.speed)}/s</span>
                {etaText && (
                  <>
                    <span className="meta-sep">/</span>
                    <span>{t("downloadItem.eta")} {etaText}</span>
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
          {isDownloading ? (
            <button type="button" className="icon-btn" onClick={(e) => { stop(e); call(() => invoke("pause_download", { id: item.id }), t("downloadItem.action_failed")); }} title={t("downloadItem.pause")} aria-label={t("downloadItem.pause")}>
              <Pause size={14} />
            </button>
          ) : !isCompleted ? (
            <button type="button" className="icon-btn" onClick={(e) => { stop(e); call(() => invoke("resume_download", { id: item.id }), t("downloadItem.action_failed")); }} title={t("downloadItem.resume")} aria-label={t("downloadItem.resume")}>
              <Play size={14} />
            </button>
          ) : null}

          <button type="button" className="icon-btn" onClick={(e) => { stop(e); invoke("open_folder", { path: fileFullPath(item) }).catch(() => showToast("error", t("downloadItem.action_failed"))); }} title={t("downloadItem.open_folder")} aria-label={t("downloadItem.open_folder")}>
            <Folder size={14} />
          </button>

          {item.status === "Queued" && (
            <>
              <button type="button" className="icon-btn" onClick={(e) => { stop(e); call(() => invoke("change_priority", { id: item.id, increase: true }), t("downloadItem.action_failed")); }} title={t("downloadItem.increase_priority")} aria-label={t("downloadItem.increase_priority")}>
                <ChevronUp size={14} />
              </button>
              <button type="button" className="icon-btn" onClick={(e) => { stop(e); call(() => invoke("change_priority", { id: item.id, increase: false }), t("downloadItem.action_failed")); }} title={t("downloadItem.decrease_priority")} aria-label={t("downloadItem.decrease_priority")}>
                <ChevronDown size={14} />
              </button>
            </>
          )}

          <button type="button" className="icon-btn danger" onClick={(e) => {
            stop(e);
            setConfirmRemove(true);
          }} title={t("downloadItem.delete")} aria-label={t("downloadItem.delete")}>
            <Trash2 size={14} />
          </button>
        </div>
      </div>

      {menu && (
        <div className="ctx-menu" style={{ left: menu.x, top: menu.y }} role="menu">
          {isDownloading ? (
            <button type="button" role="menuitem" onClick={() => call(() => invoke("pause_download", { id: item.id }), t("downloadItem.action_failed"))}>{t("downloadItem.pause")}</button>
          ) : !isCompleted ? (
            <button type="button" role="menuitem" onClick={() => call(() => invoke("resume_download", { id: item.id }), t("downloadItem.action_failed"))}>{t("downloadItem.resume")}</button>
          ) : null}
          <button type="button" role="menuitem" onClick={() => invoke("open_folder", { path: fileFullPath(item) }).catch(() => showToast("error", t("downloadItem.action_failed")))}>{t("downloadItem.open_folder")}</button>
          <button type="button" role="menuitem" onClick={() => navigator.clipboard.writeText(item.url)}><Copy size={12} /> {t("downloadItem.copy_url")}</button>
          <button type="button" role="menuitem" className="danger" onClick={() => setConfirmRemove(true)}>{t("downloadItem.delete")}</button>
        </div>
      )}

      {confirmRemove && (
        <ConfirmDialog
          message={t("inspector.confirm_remove")}
          onConfirm={doRemove}
          onCancel={() => setConfirmRemove(false)}
        />
      )}
    </>
  );
}
