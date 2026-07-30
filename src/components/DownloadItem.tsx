interface DownloadItemProps {
  item: {
    id: number;
    filename: string;
    progress: number;
    speed: string;
    size: string;
    status: string;
  };
}

export default function DownloadItem({ item }: DownloadItemProps) {
  return (
    <div className="download-item">
      <div className="item-filename" title={item.filename}>
        {item.filename}
      </div>
      <div className="item-progress-col">
        <div className="progress-bar-bg">
          <div 
            className="progress-bar-fill" 
            style={{ width: `${item.progress}%` }} 
          />
        </div>
        <div className="progress-text">{item.progress}%</div>
      </div>
      <div className="item-speed">{item.speed}</div>
      <div className="item-size">{item.size}</div>
      <div className="item-status">{item.status}</div>
    </div>
  );
}
