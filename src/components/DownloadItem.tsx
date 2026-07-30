import { invoke } from "@tauri-apps/api/core";
import { Play, Pause, ChevronUp, ChevronDown, File, Film, Music, Package } from "lucide-react";
import { DownloadModel } from "./DownloadList";
import SpeedGraph from "./SpeedGraph";

interface DownloadItemProps {
  item: DownloadModel;
}

function formatBytes(bytes: number, decimals = 2) {
  if (!+bytes) return "0 Bytes";
  const k = 1024;
  const dm = decimals < 0 ? 0 : decimals;
  const sizes = ["Bytes", "KB", "MB", "GB", "TB"];
  const i = Math.floor(Math.log(bytes) / Math.log(k));
  return `${parseFloat((bytes / Math.pow(k, i)).toFixed(dm))} ${sizes[i]}`;
}

export default function DownloadItem({ item }: DownloadItemProps) {
  const progressPercent =
    item.total_size > 0
      ? Math.round((item.downloaded_size / item.total_size) * 100)
      : 0;
  
  const isDownloading = item.status === "Downloading";

  const handlePriority = async (increase: boolean) => {
    try {
      await invoke("change_priority", { id: item.id, increase });
    } catch (e) {
      console.error(e);
    }
  };

  const getFileIcon = (filename: string) => {
    const lower = filename.toLowerCase();
    if (lower.match(/\.(mp4|mkv|mov|avi|webm)$/)) return <Film size={14} />;
    if (lower.match(/\.(mp3|wav|ogg|flac)$/)) return <Music size={14} />;
    if (lower.match(/\.(zip|rar|7z|tar|gz)$/)) return <Package size={14} />;
    return <File size={14} />;
  };

  return (
    <div className="download-item">
      <div className="item-filename" title={item.filename}>
        <div className="item-icon-circle">
          {getFileIcon(item.filename)}
        </div>
        {item.filename}
      </div>
      <div className="item-progress-col">
        <div className="progress-bar-bg">
          <div
            className={`progress-bar-fill ${isDownloading ? "active" : ""}`}
            style={{ width: `${progressPercent}%` }}
          />
        </div>
        <div className="progress-text">{progressPercent}%</div>
      </div>
      <div className="item-speed" style={{ display: 'flex', alignItems: 'center' }}>
        {isDownloading ? `${formatBytes(item.speed)}/s` : "-"}
        {isDownloading && <SpeedGraph speed={item.speed} />}
      </div>
      <div className="item-size">
        {formatBytes(item.total_size)}
      </div>
      <div className="item-actions">
        {item.status === "Downloading" ? (
          <button className="icon-btn" title="Pause">
            <Pause size={16} />
          </button>
        ) : (
          <button className="icon-btn" title="Resume">
            <Play size={16} />
          </button>
        )}
        {item.status === "Queued" && (
          <>
            <button className="icon-btn" onClick={() => handlePriority(true)} title="Increase Priority">
              <ChevronUp size={16} />
            </button>
            <button className="icon-btn" onClick={() => handlePriority(false)} title="Decrease Priority">
              <ChevronDown size={16} />
            </button>
          </>
        )}
      </div>
    </div>
  );
}
