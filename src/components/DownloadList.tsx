import DownloadItem from "./DownloadItem";

interface DownloadListProps {
  category: string;
}

// Dummy data
const MOCK_DOWNLOADS = [
  { id: 1, filename: "ubuntu-22.04.3-desktop-amd64.iso", progress: 45, speed: "12.5 MB/s", size: "4.7 GB", status: "downloading" },
  { id: 2, filename: "react-developer-tools.zip", progress: 100, speed: "0 B/s", size: "1.2 MB", status: "completed" },
  { id: 3, filename: "vacation-video.mp4", progress: 12, speed: "2.1 MB/s", size: "1.5 GB", status: "downloading" },
  { id: 4, filename: "financial-report-2023.pdf", progress: 0, speed: "0 B/s", size: "450 KB", status: "paused" },
];

export default function DownloadList({ category }: DownloadListProps) {
  // Simple filter for dummy data
  const filteredDownloads = MOCK_DOWNLOADS.filter(d => {
    if (category === "All Downloads") return true;
    if (category === "Downloading") return d.status === "downloading";
    if (category === "Completed") return d.status === "completed";
    if (category === "Video") return d.filename.endsWith(".mp4");
    if (category === "Documents") return d.filename.endsWith(".pdf");
    return true;
  });

  return (
    <div className="download-list-container">
      <div className="download-list-header">
        <div>Filename</div>
        <div>Progress</div>
        <div>Speed</div>
        <div>Size</div>
        <div>Status</div>
      </div>
      <div className="download-list">
        {filteredDownloads.map((dl) => (
          <DownloadItem key={dl.id} item={dl} />
        ))}
      </div>
    </div>
  );
}
