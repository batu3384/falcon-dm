import { useState } from 'react';
import { SchedulerModal } from './SchedulerModal';
import { SettingsModal } from './SettingsModal';

interface ToolbarProps {
  onAddClick: () => void;
}

export default function Toolbar({ onAddClick }: ToolbarProps) {
  const [schedulerOpen, setSchedulerOpen] = useState(false);
  const [settingsOpen, setSettingsOpen] = useState(false);

  return (
    <>
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
        <button className="toolbar-btn" onClick={() => setSchedulerOpen(true)}>
          🕒 Scheduler
        </button>
        <div style={{ flex: 1 }}></div>
        <button className="toolbar-btn" onClick={() => setSettingsOpen(true)}>
          ⚙️ Settings
        </button>
      </div>
      <SchedulerModal isOpen={schedulerOpen} onClose={() => setSchedulerOpen(false)} />
      {settingsOpen && <SettingsModal onClose={() => setSettingsOpen(false)} />}
    </>
  );
}
