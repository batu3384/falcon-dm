import { Download, Activity, CheckCircle2, Film, Music, FileText } from "lucide-react";

interface SidebarProps {
  activeCategory: string;
  onSelectCategory: (category: string) => void;
}

const CATEGORIES = [
  "All Downloads",
  "Downloading",
  "Completed",
  "Video",
  "Music",
  "Documents",
];

export default function Sidebar({ activeCategory, onSelectCategory }: SidebarProps) {
  return (
    <div className="sidebar">
      <div className="sidebar-title">Library</div>
      {CATEGORIES.map((cat) => (
        <div
          key={cat}
          className={`sidebar-item ${activeCategory === cat ? "active" : ""}`}
          onClick={() => onSelectCategory(cat)}
        >
          <span className="sidebar-item-icon">
            {cat === "All Downloads" && <Download size={16} />}
            {cat === "Downloading" && <Activity size={16} />}
            {cat === "Completed" && <CheckCircle2 size={16} />}
            {cat === "Video" && <Film size={16} />}
            {cat === "Music" && <Music size={16} />}
            {cat === "Documents" && <FileText size={16} />}
          </span>
          {cat}
        </div>
      ))}
    </div>
  );
}
