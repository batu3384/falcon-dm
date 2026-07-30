import { useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { downloadDir } from "@tauri-apps/api/path";

interface NewDownloadModalProps {
  onClose: () => void;
  onSuccess?: () => void;
  initialUrl?: string;
}

export default function NewDownloadModal({ onClose, onSuccess, initialUrl }: NewDownloadModalProps) {
  const [url, setUrl] = useState(initialUrl || "");

  const handleDownload = async () => {
    if (url.trim()) {
      try {
        const defaultPath = await downloadDir();
        await invoke("add_download", {
          url: url.trim(),
          filename: url.split("/").pop() || "download.bin",
          savePath: defaultPath,
        });
        // We don't need window.location.reload() since DownloadList listens to events.
        // We might just need a small delay or tell the frontend to refetch.
        onClose();
        if (onSuccess) onSuccess();
      } catch (error) {
        console.error("Failed to add download", error);
      }
    }
  };

  return (
    <div className="modal-overlay">
      <div className="modal-content">
        <div className="modal-title">Add New Download</div>
        <input 
          className="modal-input" 
          type="text" 
          placeholder="https://example.com/file.zip" 
          value={url}
          onChange={(e) => setUrl(e.target.value)}
          autoFocus
        />
        <div className="modal-actions">
          <button className="modal-btn" onClick={onClose}>Cancel</button>
          <button className="modal-btn primary" onClick={handleDownload}>Download</button>
        </div>
      </div>
    </div>
  );
}
