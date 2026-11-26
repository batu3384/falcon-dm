import type { SettingsModel } from '../../types';

export interface Prefs {
  /** 'system' | 'en' | 'tr' */
  language: string;
  clipboardMonitor: boolean;
}

export interface ScheduleDraft {
  active: boolean;
  start: string;
  stop: string;
}

export interface PaneProps {
  settings: SettingsModel;
  update: (patch: Partial<SettingsModel>) => void;
}
