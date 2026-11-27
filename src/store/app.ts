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
  /** Requests hidden by the user; a changed code is a new attempt. */
  dismissedPairKeys: Set<string>;
  refreshStats: () => Promise<void>;
  refreshExtension: () => Promise<void>;
  refreshSchedule: () => Promise<void>;
  showPairPrompt: (request: PairRequest) => void;
  dismissPairPrompt: () => void;
}

const pairKey = (request: PairRequest) => `${request.extension_id}:${request.code}`;

export const useAppStore = create<AppState>((set, get) => ({
  stats: null,
  statsError: null,
  extension: null,
  schedule: null,
  pairPrompt: null,
  dismissedPairKeys: new Set(),

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
      const dismissed = get().dismissedPairKeys;
      const waiting = extension.pending.find((request) => !dismissed.has(pairKey(request)));
      const current = get().pairPrompt;
      if (
        waiting &&
        (!current ||
          (current.extension_id === waiting.extension_id && current.code !== waiting.code))
      ) {
        set({ pairPrompt: waiting });
      }
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
    if (get().dismissedPairKeys.has(pairKey(request))) return;
    const current = get().pairPrompt;
    // A newer code from the same extension replaces the prompt.
    if (!current || current.extension_id === request.extension_id) set({ pairPrompt: request });
  },

  dismissPairPrompt: () => {
    set((state) => {
      const current = state.pairPrompt;
      if (!current) return { pairPrompt: null };
      return {
        pairPrompt: null,
        dismissedPairKeys: new Set(state.dismissedPairKeys).add(pairKey(current)),
      };
    });
    void get().refreshExtension();
  },
}));
