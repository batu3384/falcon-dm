import { useState } from "react";

interface NewDownloadModalProps {
  onClose: () => void;
}

export default function NewDownloadModal({ onClose }: NewDownloadModalProps) {
  const [url, setUrl] = useState("");

  const handleDownload = () => {
    // Basic validation or invoke rust backend
    if (url.trim()) {
      console.log("Add download URL:", url);
      onClose();
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
