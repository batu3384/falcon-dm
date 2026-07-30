import { useState } from 'react';
import { Plus, Play, Pause, Clock, Settings } from 'lucide-react';
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
      <div className="toolbar" data-tauri-drag-region>
        <button className="toolbar-btn primary" onClick={onAddClick}>
          <Plus size={16} /> Add URL
        </button>
        <button className="toolbar-btn">
          <Play size={16} /> Start All
        </button>
        <button className="toolbar-btn">
          <Pause size={16} /> Pause All
        </button>
        <button className="toolbar-btn" onClick={() => setSchedulerOpen(true)}>
          <Clock size={16} /> Scheduler
        </button>
        <div style={{ flex: 1 }}></div>
        <button className="toolbar-btn" onClick={() => setSettingsOpen(true)}>
          <Settings size={16} /> Settings
        </button>
      </div>
      <SchedulerModal isOpen={schedulerOpen} onClose={() => setSchedulerOpen(false)} />
      {settingsOpen && <SettingsModal onClose={() => setSettingsOpen(false)} />}
    </>
  );
}
