interface ToolbarProps {
  onAddClick: () => void;
}

export default function Toolbar({ onAddClick }: ToolbarProps) {
  return (
    <div className="toolbar">
      <button className="toolbar-btn primary" onClick={onAddClick}>
        ➕ Add URL
      </button>
      <button className="toolbar-btn">
        ▶️ Start All
      </button>
      <button className="toolbar-btn">
        ⏸ Pause All
      </button>
      <div style={{ flex: 1 }}></div>
      <button className="toolbar-btn">
        ⚙️ Settings
      </button>
    </div>
  );
}
