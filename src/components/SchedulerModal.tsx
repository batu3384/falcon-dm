import { useState } from 'react';
import { invoke } from '@tauri-apps/api/core';

interface SchedulerModalProps {
  isOpen: boolean;
  onClose: () => void;
}

export function SchedulerModal({ isOpen, onClose }: SchedulerModalProps) {
  const [active, setActive] = useState(false);
  const [startTime, setStartTime] = useState('');
  const [stopTime, setStopTime] = useState('');
  const [saving, setSaving] = useState(false);

  if (!isOpen) return null;

  const handleSave = async () => {
    setSaving(true);
    try {
      await invoke('set_schedule', {
        startTime: startTime || null,
        stopTime: stopTime || null,
        active,
      });
      onClose();
    } catch (e) {
      console.error(e);
      alert('Failed to set schedule');
    } finally {
      setSaving(false);
    }
  };

  return (
    <div className="fixed inset-0 bg-black/60 backdrop-blur-sm flex items-center justify-center z-50 animate-in fade-in duration-200">
      <div className="bg-slate-900 border border-slate-700 p-6 rounded-2xl w-96 shadow-2xl">
        <h2 className="text-xl font-bold mb-4 text-white">Queue Scheduler</h2>
        
        <div className="space-y-4 mb-6">
          <div className="flex items-center justify-between">
            <label className="text-sm font-medium text-slate-300">Enable Scheduler</label>
            <input 
              type="checkbox" 
              checked={active} 
              onChange={(e) => setActive(e.target.checked)}
              className="w-5 h-5 rounded border-slate-600 bg-slate-800 text-blue-500 focus:ring-blue-500/50"
            />
          </div>

          <div>
            <label className="block text-sm font-medium text-slate-300 mb-1">Start Time</label>
            <input 
              type="time"
              value={startTime}
              onChange={(e) => setStartTime(e.target.value)}
              disabled={!active}
              className="w-full bg-slate-800 border border-slate-700 rounded-lg px-3 py-2 text-white focus:ring-2 focus:ring-blue-500/50 focus:border-blue-500 disabled:opacity-50"
            />
          </div>

          <div>
            <label className="block text-sm font-medium text-slate-300 mb-1">Stop Time</label>
            <input 
              type="time"
              value={stopTime}
              onChange={(e) => setStopTime(e.target.value)}
              disabled={!active}
              className="w-full bg-slate-800 border border-slate-700 rounded-lg px-3 py-2 text-white focus:ring-2 focus:ring-blue-500/50 focus:border-blue-500 disabled:opacity-50"
            />
          </div>
        </div>

        <div className="flex justify-end gap-3">
          <button
            onClick={onClose}
            className="px-4 py-2 rounded-lg text-slate-300 hover:bg-slate-800 transition-colors"
          >
            Cancel
          </button>
          <button
            onClick={handleSave}
            disabled={saving}
            className="px-4 py-2 bg-blue-600 hover:bg-blue-500 text-white rounded-lg transition-colors font-medium shadow-lg shadow-blue-500/20 disabled:opacity-50"
          >
            {saving ? 'Saving...' : 'Save Schedule'}
          </button>
        </div>
      </div>
    </div>
  );
}
