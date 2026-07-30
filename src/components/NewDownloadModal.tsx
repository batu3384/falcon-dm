import { useState } from "react";
import { invoke } from "@tauri-apps/api/core";

interface NewDownloadModalProps {
  onClose: () => void;
}

export default function NewDownloadModal({ onClose }: NewDownloadModalProps) {
  const [url, setUrl] = useState("");

  const handleDownload = async () => {
    if (url.trim()) {
      try {
        await invoke("add_download", {
          url: url.trim(),
          filename: url.split("/").pop() || "download.bin",
          savePath: "/tmp",
        });
        // Force refresh somehow? Real app might use an event, but here we can just close
        onClose();
        // Since we don't have global state yet, a dirty reload works for demo,
        // or just let the user re-select the category to fetch
        window.location.reload();
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
