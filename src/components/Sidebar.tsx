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
            {/* Simple dot or generic icon placeholder */}
            {cat === "All Downloads" && "📥"}
            {cat === "Downloading" && "⏳"}
            {cat === "Completed" && "✅"}
            {cat === "Video" && "🎬"}
            {cat === "Music" && "🎵"}
            {cat === "Documents" && "📄"}
          </span>
          {cat}
        </div>
      ))}
    </div>
  );
}
