import { useEffect, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import DownloadItem from "./DownloadItem";

interface DownloadListProps {
  category: string;
}

export interface DownloadModel {
  id: number;
  filename: string;
  total_size: number;
  downloaded_size: number;
  speed: number;
  status: string;
  category: string;
}

interface ProgressPayload {
  id: number;
  downloaded_size: number;
  total_size: number;
  speed: number;
  status: string;
  connections: number;
}

export default function DownloadList({ category }: DownloadListProps) {
  const [downloads, setDownloads] = useState<DownloadModel[]>([]);

  useEffect(() => {
    // Initial fetch
    const fetchDownloads = async () => {
      try {
        const data = await invoke<DownloadModel[]>("get_downloads", {
          filter: {},
        });
        setDownloads(data);
      } catch (error) {
        console.error("Failed to fetch downloads", error);
      }
    };
    fetchDownloads();

    // Listen for progress events
    const unlisten = listen<ProgressPayload>("download-progress", (event) => {
      const payload = event.payload;
      setDownloads((prev) =>
        prev.map((d) =>
          d.id === payload.id
            ? {
                ...d,
                downloaded_size: payload.downloaded_size,
                total_size: payload.total_size,
                speed: payload.speed,
                status: payload.status,
              }
            : d
        )
      );
    });

    return () => {
      unlisten.then((f) => f());
    };
  }, []);

  const filteredDownloads = downloads.filter((d) => {
    if (category === "All Downloads") return true;
    if (category === "Downloading") return d.status === "Downloading";
    if (category === "Completed") return d.status === "Completed";
    if (category === "Video") return d.category === "Video";
    if (category === "Documents") return d.category === "Document";
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
