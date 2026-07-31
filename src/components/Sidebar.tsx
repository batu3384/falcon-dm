import { useTranslation } from "react-i18next";
import { Download, Activity, CheckCircle2, Film, Music, FileText, Package, AppWindow, Pause, AlertTriangle } from "lucide-react";

interface SidebarProps {
  activeCategory: string;
  onSelectCategory: (category: string) => void;
  counts?: Record<string, number>;
}

export default function Sidebar({ activeCategory, onSelectCategory, counts = {} }: SidebarProps) {
  const { t } = useTranslation();

  const categories = [
    { id: "All Downloads", label: t("sidebar.all_downloads"), icon: Download },
    { id: "Downloading", label: t("sidebar.active"), icon: Activity },
    { id: "Paused", label: t("sidebar.paused"), icon: Pause },
    { id: "Failed", label: t("sidebar.failed"), icon: AlertTriangle },
    { id: "Completed", label: t("sidebar.completed"), icon: CheckCircle2 },
  ];

  const types = [
    { id: "Video", label: t("sidebar.videos"), icon: Film },
    { id: "Music", label: t("sidebar.music"), icon: Music },
    { id: "Documents", label: t("sidebar.documents"), icon: FileText },
    { id: "Compressed", label: t("sidebar.compressed"), icon: Package },
    { id: "Programs", label: t("sidebar.programs"), icon: AppWindow },
  ];

  const total = counts["All Downloads"] || 0;
  const done = counts["Completed"] || 0;
  const pct = total > 0 ? Math.round((done / total) * 100) : 0;

  return (
    <aside className="sidebar no-drag" aria-label={t("sidebar.title")}>
      <div className="sidebar-head">
        <img src="/icon.png" alt="" className="brand-icon" />
        <span className="brand-name">{t("sidebar.title")}</span>
      </div>
      <div className="sidebar-scroll">
        <div className="sidebar-group">
          <div className="sidebar-label">{t("sidebar.library")}</div>
          {categories.map((cat) => {
            const Icon = cat.icon;
            return (
              <button
                key={cat.id}
                type="button"
                className={`nav-item ${activeCategory === cat.id ? "active" : ""}`}
                onClick={() => onSelectCategory(cat.id)}
                aria-current={activeCategory === cat.id ? "page" : undefined}
              >
                <Icon size={16} strokeWidth={1.75} />
                <span className="nav-text">{cat.label}</span>
                {counts[cat.id] !== undefined && counts[cat.id] > 0 && (
                  <span className="nav-badge">{counts[cat.id]}</span>
                )}
              </button>
            );
          })}
        </div>

        <div className="sidebar-group">
          <div className="sidebar-label">{t("sidebar.categories")}</div>
          {types.map((cat) => {
            const Icon = cat.icon;
            return (
              <button
                key={cat.id}
                type="button"
                className={`nav-item ${activeCategory === cat.id ? "active" : ""}`}
                onClick={() => onSelectCategory(cat.id)}
                aria-current={activeCategory === cat.id ? "page" : undefined}
              >
                <Icon size={16} strokeWidth={1.75} />
                <span className="nav-text">{cat.label}</span>
                {counts[cat.id] !== undefined && counts[cat.id] > 0 && (
                  <span className="nav-badge">{counts[cat.id]}</span>
                )}
              </button>
            );
          })}
        </div>
      </div>
      <div className="sidebar-foot">
        <div className="storage-row">
          <div className="storage-label">
            <span>{t("sidebar.completion_progress")}</span>
            <span className="mono">{done}/{total}</span>
          </div>
          <div className="storage-bar" role="progressbar" aria-valuenow={pct} aria-valuemin={0} aria-valuemax={100}>
            <div className="storage-fill" style={{ width: `${pct}%` }} />
          </div>
        </div>
      </div>
    </aside>
  );
}
