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

  return (
    <div className="download-item">
      <div className="item-filename" title={item.filename}>
        {item.filename}
      </div>
      <div className="item-progress-col">
        <div className="progress-bar-bg">
          <div
            className="progress-bar-fill"
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
      <div className="item-status">{item.status}</div>
    </div>
  );
}
