import { create } from 'zustand';
import type { DownloadModel } from '../types';

export type SettingsTab = 'general' | 'downloads' | 'network' | 'extension' | 'profiles';

interface UiState {
  /** Prefilled URL text for the New Download sheet; null = closed. */
  newDownload: string | null;
  settingsTab: SettingsTab | null;
  logsOpen: boolean;
  paletteOpen: boolean;
  onboardingOpen: boolean;
  /** Rows waiting for the remove confirmation. */
  confirmRemove: DownloadModel[] | null;
  moveTarget: DownloadModel | null;
  /** Row waiting for a fresh link (expired URL renewal). */
  refreshTarget: DownloadModel | null;
  inspectorVisible: boolean;

  openNewDownload: (prefill?: string) => void;
  closeNewDownload: () => void;
  openSettings: (tab?: SettingsTab) => void;
  closeSettings: () => void;
  setLogsOpen: (open: boolean) => void;
  setPaletteOpen: (open: boolean) => void;
  setOnboardingOpen: (open: boolean) => void;
  askRemove: (rows: DownloadModel[]) => void;
  closeRemove: () => void;
  setMoveTarget: (row: DownloadModel | null) => void;
  setRefreshTarget: (row: DownloadModel | null) => void;
  toggleInspector: () => void;
}

const INSPECTOR_KEY = 'falcon_inspector_hidden';

export const useUiStore = create<UiState>((set) => ({
  newDownload: null,
  settingsTab: null,
  logsOpen: false,
  paletteOpen: false,
  onboardingOpen: false,
  confirmRemove: null,
  moveTarget: null,
  refreshTarget: null,
  inspectorVisible: localStorage.getItem(INSPECTOR_KEY) !== '1',

  openNewDownload: (prefill = '') => set({ newDownload: prefill }),
  closeNewDownload: () => set({ newDownload: null }),
  openSettings: (tab = 'general') => set({ settingsTab: tab }),
  closeSettings: () => set({ settingsTab: null }),
  setLogsOpen: (logsOpen) => set({ logsOpen }),
  setPaletteOpen: (paletteOpen) => set({ paletteOpen }),
  setOnboardingOpen: (onboardingOpen) => set({ onboardingOpen }),
  askRemove: (rows) => set({ confirmRemove: rows.length ? rows : null }),
  closeRemove: () => set({ confirmRemove: null }),
  setMoveTarget: (moveTarget) => set({ moveTarget }),
  setRefreshTarget: (refreshTarget) => set({ refreshTarget }),
  toggleInspector: () =>
    set((state) => {
      const inspectorVisible = !state.inspectorVisible;
      localStorage.setItem(INSPECTOR_KEY, inspectorVisible ? '0' : '1');
      return { inspectorVisible };
    }),
}));
