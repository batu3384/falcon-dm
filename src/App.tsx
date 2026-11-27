import { useCallback, useEffect, useMemo, useRef, useState } from 'react';
import { useTranslation } from 'react-i18next';
import {
  AlertCircle,
  CheckCircle2,
  Download,
  Gauge,
  Info,
  Moon,
  PanelRight,
  Pause,
  Play,
  Plus,
  Puzzle,
  ScrollText,
  Settings,
  X,
} from 'lucide-react';
import './App.css';
import Sidebar from './components/Sidebar';
import Toolbar from './components/Toolbar';
import DownloadList from './components/DownloadList';
import StatusBar from './components/StatusBar';
import { Inspector } from './components/Inspector';
import NewDownloadSheet from './components/NewDownloadSheet';
import { SettingsSheet, CLIPBOARD_KEY } from './components/settings/SettingsSheet';
import { OnboardingWizard, ONBOARDING_KEY } from './components/OnboardingWizard';
import { PairRequestDialog } from './components/PairRequestDialog';
import { MoveRenameDialog } from './components/MoveRenameDialog';
import { LogPanel } from './components/LogPanel';
import { CommandPalette, type PaletteAction } from './components/CommandPalette';
import { ConfirmDialog } from './components/ConfirmDialog';
import { ErrorBoundary } from './components/ErrorBoundary';
import { useDownloadsStore } from './store/downloads';
import { useAppStore } from './store/app';
import { useUiStore } from './store/ui';
import { useToastStore } from './store/toast';
import {
  onDownloadAdded,
  onDownloadProgress,
  onDownloadRemoved,
  onDownloadUpdated,
  onPairRequest,
} from './api/events';
import * as api from './api/commands';
import { applyTheme, formatSpeed, watchSystemTheme } from './types';
import { useClipboardMonitor } from './hooks/useClipboardMonitor';
import { isForegroundDialogOpen } from './hooks/useModalA11y';
import { publicHttpUrlsFromMixedText } from './lib/publicHttpUrl';
import { toggleSpeedLimit } from './lib/speedLimit';
import { useDownloadActions } from './lib/useDownloadActions';

function useDebounced(fn: () => void, ms: number) {
  const timer = useRef<number | undefined>(undefined);
  return useCallback(() => {
    window.clearTimeout(timer.current);
    timer.current = window.setTimeout(fn, ms);
  }, [fn, ms]);
}

function App() {
  const { t, i18n } = useTranslation();
  const showToast = useToastStore((s) => s.showToast);
  const toasts = useToastStore((s) => s.toasts);
  const dismissToast = useToastStore((s) => s.dismiss);

  const rows = useDownloadsStore((s) => s.rows);
  const selectedIds = useDownloadsStore((s) => s.selectedIds);
  const primaryId = useDownloadsStore((s) => s.primaryId);
  const refresh = useDownloadsStore((s) => s.refresh);
  const upsert = useDownloadsStore((s) => s.upsert);
  const remove = useDownloadsStore((s) => s.remove);
  const applyProgress = useDownloadsStore((s) => s.applyProgress);

  const refreshStats = useAppStore((s) => s.refreshStats);
  const refreshExtension = useAppStore((s) => s.refreshExtension);
  const refreshSchedule = useAppStore((s) => s.refreshSchedule);
  const showPairPrompt = useAppStore((s) => s.showPairPrompt);
  const pairPrompt = useAppStore((s) => s.pairPrompt);
  const stats = useAppStore((s) => s.stats);

  const ui = useUiStore();
  const actions = useDownloadActions();
  const [dragging, setDragging] = useState(false);
  const [clipboardMonitor, setClipboardMonitor] = useState(
    () => localStorage.getItem(CLIPBOARD_KEY) === '1',
  );
  const statsLater = useDebounced(
    useCallback(() => void refreshStats(), [refreshStats]),
    300,
  );

  // First load, theme and onboarding.
  useEffect(() => {
    watchSystemTheme();
    api
      .getSettings()
      .then((s) => applyTheme(s.theme))
      .catch((e) => {
        applyTheme('system');
        const msg = api.extractTauriError(e);
        if (msg.includes('corrupt')) showToast('error', t('errors.settings_corrupt'));
      });
    if (!localStorage.getItem(ONBOARDING_KEY)) useUiStore.getState().setOnboardingOpen(true);
    void refresh();
    void refreshStats();
    void refreshExtension();
    void refreshSchedule();
  }, [refresh, refreshStats, refreshExtension, refreshSchedule, showToast, t]);

  // Live events from the engine.
  useEffect(() => {
    const unlisten = [
      onDownloadAdded((row) => {
        upsert(row);
        statsLater();
      }),
      onDownloadUpdated((row) => {
        upsert(row);
        statsLater();
      }),
      onDownloadRemoved((id) => {
        remove(id);
        statsLater();
      }),
      onDownloadProgress(applyProgress),
      onPairRequest((request) => {
        showPairPrompt(request);
        void refreshExtension();
      }),
    ];
    return () => {
      unlisten.forEach((p) => void p.then((off) => off()));
    };
  }, [upsert, remove, applyProgress, showPairPrompt, refreshExtension, statsLater]);

  // Safety net: events cover changes, a slow resync covers anything missed.
  useEffect(() => {
    const statsTimer = window.setInterval(() => void refreshStats(), 5000);
    const listTimer = window.setInterval(() => void refresh(), 20000);
    const onFocus = () => {
      void refresh();
      void refreshStats();
      void refreshExtension();
    };
    window.addEventListener('focus', onFocus);
    return () => {
      window.clearInterval(statsTimer);
      window.clearInterval(listTimer);
      window.removeEventListener('focus', onFocus);
    };
  }, [refresh, refreshStats, refreshExtension]);

  useEffect(() => {
    document.documentElement.lang = i18n.language;
    void api.setUiLanguage(i18n.language).catch(() => {});
  }, [i18n.language]);

  const pauseAll = useCallback(async () => {
    try {
      const count = await api.pauseAllDownloads();
      showToast('info', t('app.paused_count', { count }));
    } catch (e) {
      showToast('error', api.extractTauriError(e));
    }
    void refreshStats();
  }, [refreshStats, showToast, t]);

  const resumeAll = useCallback(async () => {
    try {
      const count = await api.resumeAllDownloads();
      showToast('info', t('app.resumed_count', { count }));
    } catch (e) {
      showToast('error', api.extractTauriError(e));
    }
    void refreshStats();
  }, [refreshStats, showToast, t]);

  const onToggleSpeedLimit = useCallback(async () => {
    try {
      const kbps = await toggleSpeedLimit();
      showToast(
        'info',
        kbps > 0
          ? t('app.limit_on', { limit: formatSpeed(kbps * 1000, i18n.language) })
          : t('app.limit_off'),
      );
    } catch (e) {
      showToast('error', api.extractTauriError(e));
    }
    void refreshStats();
  }, [i18n.language, refreshStats, showToast, t]);

  // App-wide shortcuts. The list handles its own keys while focused.
  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      if (!(e.metaKey || e.ctrlKey) || e.altKey) return;
      const key = e.key.toLowerCase();
      const state = useUiStore.getState();
      if (key === 'k') {
        e.preventDefault();
        if (state.paletteOpen) state.setPaletteOpen(false);
        else if (!isForegroundDialogOpen()) state.setPaletteOpen(true);
        return;
      }
      if (isForegroundDialogOpen()) return;
      if (key === 'n') {
        e.preventDefault();
        state.openNewDownload();
      } else if (key === ',') {
        e.preventDefault();
        state.openSettings();
      }
    };
    window.addEventListener('keydown', onKey);
    return () => window.removeEventListener('keydown', onKey);
  }, []);

  useClipboardMonitor(
    clipboardMonitor,
    (text) => {
      if (isForegroundDialogOpen()) return false;
      useUiStore.getState().openNewDownload(text);
    },
    () => showToast('error', t('app.clipboard_failed')),
  );

  const onDrop = (e: React.DragEvent) => {
    e.preventDefault();
    setDragging(false);
    if (isForegroundDialogOpen()) return;
    const text = e.dataTransfer.getData('text/uri-list') || e.dataTransfer.getData('text/plain');
    const urls = publicHttpUrlsFromMixedText(text);
    if (urls.length) ui.openNewDownload(urls.join('\n'));
    else if (text.trim()) showToast('error', t('new_download.invalid_url'));
  };

  const selectedRows = useMemo(
    () => rows.filter((row) => selectedIds.has(row.id)),
    [rows, selectedIds],
  );
  const primary = rows.find((row) => row.id === primaryId) ?? null;

  const paletteActions = useMemo<PaletteAction[]>(
    () => [
      {
        id: 'new',
        label: t('toolbar.new_download'),
        icon: Plus,
        shortcut: '⌘N',
        run: () => ui.openNewDownload(),
      },
      { id: 'resume-all', label: t('toolbar.resume_all'), icon: Play, run: resumeAll },
      { id: 'pause-all', label: t('toolbar.pause_all'), icon: Pause, run: pauseAll },
      {
        id: 'limit',
        label: t('toolbar.speed_limit'),
        icon: Gauge,
        keywords: 'speed limit hız',
        run: onToggleSpeedLimit,
      },
      {
        id: 'inspector',
        label: t('toolbar.inspector'),
        icon: PanelRight,
        run: () => ui.toggleInspector(),
      },
      {
        id: 'settings',
        label: t('settings.title'),
        icon: Settings,
        shortcut: '⌘,',
        run: () => ui.openSettings(),
      },
      {
        id: 'extension',
        label: t('settings.tab_extension'),
        icon: Puzzle,
        run: () => ui.openSettings('extension'),
      },
      { id: 'logs', label: t('logs.title'), icon: ScrollText, run: () => ui.setLogsOpen(true) },
      {
        id: 'theme',
        label: t('palette.toggle_theme'),
        icon: Moon,
        run: async () => {
          const next =
            document.documentElement.getAttribute('data-theme') === 'dark' ? 'light' : 'dark';
          applyTheme(next);
          const s = await api.getSettings();
          await api.saveSettings({ ...s, theme: next });
        },
      },
    ],
    [t, ui, resumeAll, pauseAll, onToggleSpeedLimit],
  );

  const removing = ui.confirmRemove;
  const confirmRemove = async (deleteFile?: boolean) => {
    const targets = removing ?? [];
    ui.closeRemove();
    const results = await Promise.allSettled(
      targets.map((row) => api.removeDownload(row.id, !!deleteFile && row.status === 'Completed')),
    );
    const failed = results.filter((r): r is PromiseRejectedResult => r.status === 'rejected');
    if (failed.length) showToast('error', api.extractTauriError(failed[0].reason));
    void refreshStats();
  };

  const showInspector = ui.inspectorVisible && selectedRows.length > 0;
  const activeCount = stats?.active ?? 0;

  return (
    <div
      className="app"
      onDragOver={(e) => {
        if (isForegroundDialogOpen() || e.dataTransfer.types.includes('Files')) return;
        e.preventDefault();
        if (!dragging) setDragging(true);
      }}
      onDragLeave={(e) => {
        if (e.relatedTarget === null) setDragging(false);
      }}
      onDrop={onDrop}
    >
      <a href="#downloads" className="skip-link">
        {t('app.skip_to_list')}
      </a>
      <Sidebar />
      <main className="main" id="downloads">
        <Toolbar
          onPauseAll={pauseAll}
          onResumeAll={resumeAll}
          onToggleSpeedLimit={onToggleSpeedLimit}
        />
        <div className="content">
          <section className="list-pane" aria-label={t('app.downloads')}>
            <ErrorBoundary label="DownloadList">
              <DownloadList />
            </ErrorBoundary>
          </section>
          {showInspector && (
            <ErrorBoundary label="Inspector">
              <Inspector
                rows={selectedRows}
                primary={primary}
                actions={actions}
                onClose={() => ui.toggleInspector()}
              />
            </ErrorBoundary>
          )}
        </div>
        <StatusBar />
      </main>

      <span className="sr-only" role="status" aria-live="polite">
        {activeCount > 0 ? t('app.live_summary', { count: activeCount }) : ''}
      </span>

      {ui.newDownload !== null && (
        <NewDownloadSheet initialText={ui.newDownload} onClose={ui.closeNewDownload} />
      )}
      {ui.settingsTab && (
        <SettingsSheet
          initialTab={ui.settingsTab}
          onClose={ui.closeSettings}
          onSaved={() => {
            setClipboardMonitor(localStorage.getItem(CLIPBOARD_KEY) === '1');
            void refreshStats();
            void refreshSchedule();
          }}
        />
      )}
      {ui.logsOpen && <LogPanel onClose={() => ui.setLogsOpen(false)} />}
      {ui.onboardingOpen && <OnboardingWizard onClose={() => ui.setOnboardingOpen(false)} />}
      {ui.paletteOpen && (
        <CommandPalette
          actions={paletteActions}
          onClose={() => ui.setPaletteOpen(false)}
          onError={(e) => showToast('error', api.extractTauriError(e))}
        />
      )}
      {ui.moveTarget && (
        <MoveRenameDialog row={ui.moveTarget} onClose={() => ui.setMoveTarget(null)} />
      )}
      {removing && (
        <ConfirmDialog
          message={
            removing.length === 1
              ? t('remove.title_one', { name: removing[0].filename })
              : t('remove.title_many', { count: removing.length })
          }
          detail={t('remove.detail')}
          deleteFileOption={removing.some((row) => row.status === 'Completed')}
          confirmLabel={t('actions.remove')}
          onConfirm={(deleteFile) => void confirmRemove(deleteFile)}
          onCancel={ui.closeRemove}
        />
      )}
      {pairPrompt && <PairRequestDialog request={pairPrompt} />}

      {toasts.length > 0 && (
        <div className="toasts" role="region" aria-label={t('app.notifications')}>
          {toasts.map((toast) => (
            <div
              key={toast.id}
              className={`toast ${toast.kind}`}
              role={toast.kind === 'error' ? 'alert' : 'status'}
            >
              {toast.kind === 'success' ? (
                <CheckCircle2 size={15} aria-hidden={true} />
              ) : toast.kind === 'error' ? (
                <AlertCircle size={15} aria-hidden={true} />
              ) : (
                <Info size={15} aria-hidden={true} />
              )}
              <span>{toast.msg}</span>
              {toast.action && (
                <button
                  type="button"
                  className="toast-action"
                  onClick={() => {
                    dismissToast(toast.id);
                    toast.action?.run();
                  }}
                >
                  {toast.action.label}
                </button>
              )}
              <button
                type="button"
                aria-label={t('common.dismiss')}
                onClick={() => dismissToast(toast.id)}
              >
                <X size={12} aria-hidden={true} />
              </button>
            </div>
          ))}
        </div>
      )}

      {dragging && (
        <div className="drop-overlay" role="status">
          <div>
            <Download size={34} aria-hidden={true} />
            {t('app.drop_hint')}
          </div>
        </div>
      )}
    </div>
  );
}

export default function AppRoot() {
  return (
    <ErrorBoundary label="AppRoot">
      <App />
    </ErrorBoundary>
  );
}
