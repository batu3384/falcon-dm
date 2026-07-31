import { useMemo } from "react";
import { useTranslation } from "react-i18next";
import { Inbox } from "lucide-react";
import DownloadItem from "./DownloadItem";
import type { DownloadModel } from "../types";

type ToastFn = (kind: "success" | "error" | "info", msg: string) => void;

interface DownloadListProps {
  downloads: DownloadModel[];
  category: string;
  searchQuery: string;
  selectedId: number | null;
  onSelectDownload: (d: DownloadModel | null) => void;
  onRefresh: () => void;
  showToast: ToastFn;
  onAddClick?: () => void;
}

export default function DownloadList({
  downloads,
  category,
  searchQuery,
  selectedId,
  onSelectDownload,
  onRefresh,
  showToast,
  onAddClick,
}: DownloadListProps) {
  const { t } = useTranslation();

  const filtered = useMemo(() => {
    const q = searchQuery.trim().toLowerCase();
    return downloads.filter((d) => {
      if (q && !d.filename.toLowerCase().includes(q) && !d.url.toLowerCase().includes(q)) return false;
      if (category === "All Downloads") return true;
      if (category === "Downloading") return d.status === "Downloading" || d.status === "Merging";
      if (category === "Completed") return d.status === "Completed";
      if (category === "Paused") return d.status === "Paused" || d.status === "Queued";
      if (category === "Failed") return d.status === "Failed";
      if (category === "Video") return d.category === "Video";
      if (category === "Music") return d.category === "Music";
      if (category === "Documents") return d.category === "Document";
      if (category === "Compressed") return d.category === "Archive";
      if (category === "Programs") return d.category === "Program";
      return true;
    });
  }, [downloads, category, searchQuery]);

  const titleMap: Record<string, string> = {
    "All Downloads": t("sidebar.all_downloads"),
    "Downloading": t("sidebar.active"),
    "Completed": t("sidebar.completed"),
    "Video": t("sidebar.videos"),
    "Music": t("sidebar.music"),
    "Documents": t("sidebar.documents"),
    "Compressed": t("sidebar.compressed"),
    "Programs": t("sidebar.programs"),
  };

  const activeCount = downloads.filter((d) => d.status === "Downloading").length;

  return (
    <div className="download-view">
      <div className="view-head">
        <h2 className="view-title">{titleMap[category] || category}</h2>
        <span className="view-count">{filtered.length}</span>
        {activeCount > 0 && (
          <span className="live-pill">
            <span className="pulse-dot" />
            {activeCount} {t("sidebar.active").toLowerCase()}
          </span>
        )}
      </div>

      <div className="dl-list">
        {filtered.length === 0 ? (
          <div className="empty-state">
            <div className="empty-icon">
              <Inbox strokeWidth={1.5} />
            </div>
            <div className="empty-title">
              {searchQuery ? t("downloadList.no_search_results") : t("downloadList.no_downloads")}
            </div>
            <div className="empty-desc">
              {searchQuery ? t("downloadList.no_search_desc") : t("downloadList.no_downloads_desc")}
            </div>
            {!searchQuery && onAddClick && (
              <button type="button" className="btn-primary" style={{ marginTop: 12 }} onClick={onAddClick}>
                {t("downloadList.add_cta")}
              </button>
            )}
          </div>
        ) : (
          filtered.map((dl) => (
            <DownloadItem
              key={dl.id}
              item={dl}
              isSelected={selectedId === dl.id}
              onSelect={() => onSelectDownload(selectedId === dl.id ? null : dl)}
              onRefresh={onRefresh}
              showToast={showToast}
            />
          ))
        )}
      </div>
    </div>
  );
}
