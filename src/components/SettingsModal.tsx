import { useState, useEffect } from 'react';
import { invoke } from '@tauri-apps/api/core';
import { open } from '@tauri-apps/plugin-dialog';

interface Settings {
  theme: string;
  default_download_path: string;
  max_concurrent_downloads: number;
  max_connections_per_server: number;
  proxy: string | null;
}

interface SettingsModalProps {
  onClose: () => void;
}

export const SettingsModal = ({ onClose }: SettingsModalProps) => {
  const [activeTab, setActiveTab] = useState<'general' | 'network'>('general');
  const [settings, setSettings] = useState<Settings>({
    theme: 'system',
    default_download_path: '~/Downloads',
    max_concurrent_downloads: 3,
    max_connections_per_server: 16,
    proxy: null,
  });

  useEffect(() => {
    invoke<Settings>('get_settings')
      .then((s) => {
        setSettings(s);
      })
      .catch((e) => console.error('Failed to load settings:', e));
  }, []);

  const handleChange = (field: keyof Settings, value: any) => {
    setSettings((prev) => ({ ...prev, [field]: value }));
  };

  const handleBrowse = async () => {
    const selected = await open({
      directory: true,
      multiple: false,
    });
    if (selected) {
      handleChange('default_download_path', selected as string);
    }
  };

  const handleSave = async () => {
    try {
      await invoke('save_settings', { settings });
      // Apply theme
      if (settings.theme === 'dark') {
        document.documentElement.classList.add('dark');
      } else if (settings.theme === 'light') {
        document.documentElement.classList.remove('dark');
      } else {
        // system
        if (window.matchMedia('(prefers-color-scheme: dark)').matches) {
          document.documentElement.classList.add('dark');
        } else {
          document.documentElement.classList.remove('dark');
        }
      }
      onClose();
    } catch (e) {
      console.error('Failed to save settings:', e);
    }
  };

  return (
    <div className="fixed inset-0 z-50 flex items-center justify-center bg-black bg-opacity-50 text-foreground">
      <div className="bg-background w-full max-w-md rounded-lg shadow-xl overflow-hidden border border-border flex flex-col max-h-[90vh]">
        <div className="p-4 border-b border-border flex justify-between items-center">
          <h2 className="text-xl font-semibold">Settings</h2>
          <button onClick={onClose} className="text-gray-500 hover:text-gray-700 dark:hover:text-gray-300">
            &times;
          </button>
        </div>
        
        <div className="flex border-b border-border">
          <button
            className={`flex-1 py-2 text-center font-medium ${activeTab === 'general' ? 'border-b-2 border-primary text-primary' : 'text-gray-500 hover:bg-muted'}`}
            onClick={() => setActiveTab('general')}
          >
            General
          </button>
          <button
            className={`flex-1 py-2 text-center font-medium ${activeTab === 'network' ? 'border-b-2 border-primary text-primary' : 'text-gray-500 hover:bg-muted'}`}
            onClick={() => setActiveTab('network')}
          >
            Network
          </button>
        </div>

        <div className="p-6 overflow-y-auto flex-1">
          {activeTab === 'general' && (
            <div className="space-y-4">
              <div>
                <label className="block text-sm font-medium mb-1">Theme</label>
                <select
                  value={settings.theme}
                  onChange={(e) => handleChange('theme', e.target.value)}
                  className="w-full rounded-md border border-input bg-background px-3 py-2 text-sm"
                >
                  <option value="system">System</option>
                  <option value="light">Light</option>
                  <option value="dark">Dark</option>
                </select>
              </div>

              <div>
                <label className="block text-sm font-medium mb-1">Default Download Path</label>
                <div className="flex space-x-2">
                  <input
                    type="text"
                    value={settings.default_download_path}
                    onChange={(e) => handleChange('default_download_path', e.target.value)}
                    className="flex-1 rounded-md border border-input bg-background px-3 py-2 text-sm"
                  />
                  <button
                    onClick={handleBrowse}
                    className="px-4 py-2 bg-secondary text-secondary-foreground rounded-md hover:bg-secondary/80 text-sm font-medium"
                  >
                    Browse
                  </button>
                </div>
              </div>
            </div>
          )}

          {activeTab === 'network' && (
            <div className="space-y-4">
              <div>
                <label className="block text-sm font-medium mb-1">Max Concurrent Downloads ({settings.max_concurrent_downloads})</label>
                <input
                  type="range"
                  min="1"
                  max="10"
                  value={settings.max_concurrent_downloads}
                  onChange={(e) => handleChange('max_concurrent_downloads', parseInt(e.target.value))}
                  className="w-full"
                />
              </div>

              <div>
                <label className="block text-sm font-medium mb-1">Max Connections Per Server</label>
                <input
                  type="number"
                  min="1"
                  max="32"
                  value={settings.max_connections_per_server}
                  onChange={(e) => handleChange('max_connections_per_server', parseInt(e.target.value))}
                  className="w-full rounded-md border border-input bg-background px-3 py-2 text-sm"
                />
              </div>

              <div>
                <label className="block text-sm font-medium mb-1">Proxy</label>
                <input
                  type="text"
                  placeholder="e.g. http://127.0.0.1:8080"
                  value={settings.proxy || ''}
                  onChange={(e) => handleChange('proxy', e.target.value || null)}
                  className="w-full rounded-md border border-input bg-background px-3 py-2 text-sm"
                />
              </div>
            </div>
          )}
        </div>

        <div className="p-4 border-t border-border flex justify-end space-x-2 bg-muted/50">
          <button
            onClick={onClose}
            className="px-4 py-2 rounded-md hover:bg-muted text-sm font-medium"
          >
            Cancel
          </button>
          <button
            onClick={handleSave}
            className="px-4 py-2 bg-primary text-primary-foreground rounded-md hover:bg-primary/90 text-sm font-medium"
          >
            Save
          </button>
        </div>
      </div>
    </div>
  );
};
