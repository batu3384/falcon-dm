import { create } from 'zustand';
import * as api from '../api/commands';
import type { DownloadStats, ExtensionStatus, PairRequest, ScheduleModel } from '../types';

interface AppState {
  stats: DownloadStats | null;
  statsError: string | null;
  extension: ExtensionStatus | null;
  schedule: ScheduleModel | null;
  /** Pairing request the user still has to answer. */
  pairPrompt: PairRequest | null;
  refreshStats: () => Promise<void>;
  refreshExtension: () => Promise<void>;
  refreshSchedule: () => Promise<void>;
  showPairPrompt: (request: PairRequest) => void;
  dismissPairPrompt: () => void;
}

export const useAppStore = create<AppState>((set, get) => ({
  stats: null,
  statsError: null,
  extension: null,
  schedule: null,
  pairPrompt: null,

  refreshStats: async () => {
    try {
      set({ stats: await api.getStats(), statsError: null });
    } catch (e) {
      set({ statsError: api.extractTauriError(e) });
    }
  },

  refreshExtension: async () => {
    try {
      const extension = await api.getExtensionStatus();
      set({ extension });
      // A request made while the window was closed still needs an answer.
      const waiting = extension.pending[0];
      if (waiting && !get().pairPrompt) set({ pairPrompt: waiting });
    } catch {
      set({ extension: null });
    }
  },

  refreshSchedule: async () => {
    try {
      set({ schedule: await api.getSchedule() });
    } catch {
      set({ schedule: null });
    }
  },

  showPairPrompt: (request) => {
    const current = get().pairPrompt;
    // A newer code from the same extension replaces the prompt.
    if (!current || current.extension_id === request.extension_id) set({ pairPrompt: request });
  },

  dismissPairPrompt: () => {
    set({ pairPrompt: null });
    void get().refreshExtension();
  },
}));
