import { useState, useEffect, useCallback, useMemo } from 'react';
import { useTranslation } from 'react-i18next';
import {
  CheckCircle2,
  AlertCircle,
  Info,
  Plus,
  Play,
  Pause,
  Clock,
  Settings,
  Gauge,
  Sun,
  Moon,
  DownloadCloud,
  Trash2,
  ScrollText,
  BarChart3,
  X,
} from 'lucide-react';
import './App.css';
import TitleBar from './components/TitleBar';
import Sidebar from './components/Sidebar';
import Toolbar from './components/Toolbar';
import DownloadList from './components/DownloadList';
import NewDownloadModal from './components/NewDownloadModal';
import { OnboardingWizard } from './components/OnboardingWizard';
import { InspectorPanel } from './components/InspectorPanel';
import StatusBar from './components/StatusBar';
import { CommandPalette, type PaletteAction } from './components/CommandPalette';
import { SchedulerModal } from './components/SchedulerModal';
import { SettingsModal } from './components/SettingsModal';
import { LogPanel } from './components/LogPanel';
import { StatsPanel } from './components/StatsPanel';
import { ErrorBoundary } from './components/ErrorBoundary';
import { ConfirmDialog } from './components/ConfirmDialog';
import { useDownloadsStore } from './store/downloads';
import { useToastStore } from './store/toast';
import { onDownloadAdded, onDownloadProgress, onPairRequest } from './api/events';
import * as api from './api/commands';
import { applyTheme, watchSystemTheme } from './types';
import { getDownloadCapabilities } from './lib/downloadCapabilities';
import { useClipboardMonitor } from './hooks/useClipboardMonitor';

function App() {
  const { t, i18n } = useTranslation();
  const showToast = useToastStore((s) => s.showToast);

  // Downloads store absorbs downloads/selection/loading state.
  const {
    downloads,
    loading,
    error,
    selectedDownload,
    selectedIds,
    hasMore,
    loadingMore,
    fetchDownloads,
    loadMoreDownloads,
    retryFetch,
    applyProgress,
    addDownload,
    selectDownload,
    clearSelection,
    syncSelectedFromList,
  } = useDownloadsStore();

  // Local UI state (not shared enough to deserve a store).
  const [showOnboarding, setShowOnboarding] = useState(false);
  const [isModalOpen, setIsModalOpen] = useState(false);
  const [clipboardMonitor, setClipboardMonitor] = useState(false);
  const [prefilledUrl, setPrefilledUrl] = useState('');
  const [activeCategory, setActiveCategory] = useState('All Downloads');
  const [searchQuery, setSearchQuery] = useState('');
  const [paletteOpen, setPaletteOpen] = useState(false);
  const [dragging, setDragging] = useState(false);
  const [speedLimited, setSpeedLimited] = useState(false);
  const [savedSpeedLimit, setSavedSpeedLimit] = useState(0);
  const [settingsOpen, setSettingsOpen] = useState(false);
  const [schedulerOpen, setSchedulerOpen] = useState(false);
  const [logsOpen, setLogsOpen] = useState(false);
  const [statsOpen, setStatsOpen] = useState(false);
  const [confirmBatchDelete, setConfirmBatchDelete] = useState(false);

  // Keep inspector in sync with live list (was a useEffect in App before).
  useEffect(() => {
    syncSelectedFromList();
  }, [downloads, syncSelectedFromList]);

  useEffect(() => {
    const onboardingComplete = localStorage.getItem('onboarding_complete');
    if (!onboardingComplete) setShowOnboarding(true);

    api
      .getSettings()
      .then((s) => applyTheme(s.theme))
      .catch(() => applyTheme('system'));
    watchSystemTheme();

    fetchDownloads();

    const refreshTimer = setInterval(fetchDownloads, 5000);

    const unlistenAdded = onDownloadAdded((d) => {
      addDownload(d);
      showToast('success', t('app.download_captured', { name: d.filename }));
    });

    const unlistenProgress = onDownloadProgress((p) => {
      applyProgress(p);
    });

    const unlistenPair = onPairRequest((extensionId) => {
      showToast('info', t('settings.pair_pending') + ': ' + extensionId);
    });

    return () => {
      unlistenAdded.then((f) => f());
      unlistenProgress.then((f) => f());
      unlistenPair.then((f) => f());
      clearInterval(refreshTimer);
    };
  }, [fetchDownloads, applyProgress, addDownload, showToast, t]);

  // ponytail: refetch with the right archived flag when the sidebar category
  // switches to/from "Archived" (archived rows are excluded by default).
  useEffect(() => {
    fetchDownloads(activeCategory === 'Archived');
  }, [activeCategory, fetchDownloads]);

  useClipboardMonitor(clipboardMonitor, (text) => {
    setPrefilledUrl(text);
    setIsModalOpen(true);
    showToast('info', t('app.clipboard_url'));
  });

  const categoryCounts = useMemo<Record<string, number>>(
    () => ({
      'All Downloads': downloads.length,
      Downloading: downloads.filter((d) => d.status === 'Downloading' || d.status === 'Merging')
        .length,
      Completed: downloads.filter((d) => d.status === 'Completed').length,
      Paused: downloads.filter((d) => d.status === 'Paused' || d.status === 'Queued').length,
      Failed: downloads.filter((d) => d.status === 'Failed').length,
      Video: downloads.filter((d) => d.category === 'Video').length,
      Music: downloads.filter((d) => d.category === 'Music').length,
      Documents: downloads.filter((d) => d.category === 'Document').length,
      Compressed: downloads.filter((d) => d.category === 'Archive').length,
      Programs: downloads.filter((d) => d.category === 'Program').length,
    }),
    [downloads],
  );

  const runBatchAction = useCallback(
    async (targets: typeof downloads, action: 'pause' | 'resume' | 'delete') => {
      if (!targets.length) return;
      const results = await Promise.allSettled(
        targets.map((download) => {
          if (action === 'delete') return api.removeDownload(download.id);
          if (action === 'pause') return api.pauseDownload(download.id);
          return api.resumeDownload(download.id);
        }),
      );
      const succeeded = results.filter((result) => result.status === 'fulfilled');
      const failures = results.flatMap((result, index) =>
        result.status === 'rejected'
          ? [{ download: targets[index], error: api.extractTauriError(result.reason) }]
          : [],
      );
      await fetchDownloads();
      const failedNames = failures
        .map(({ download, error }) => `${download.filename} (${error})`)
        .join(', ');
      showToast(
        failures.length ? 'error' : 'info',
        t('app.batch_result', {
          success: succeeded.length,
          failed: failures.length,
          names: failedNames ? `: ${failedNames}` : '',
        }),
      );
    },
    [fetchDownloads, showToast, t],
  );

  const handlePauseAll = useCallback(async () => {
    await runBatchAction(
      downloads.filter((download) => getDownloadCapabilities(download.status).pause),
      'pause',
    );
  }, [downloads, runBatchAction]);

  const handleResumeAll = useCallback(async () => {
    await runBatchAction(
      downloads.filter((download) => getDownloadCapabilities(download.status).resume),
      'resume',
    );
  }, [downloads, runBatchAction]);

  const handleBatchAction = useCallback(
    async (action: 'pause' | 'resume' | 'delete') => {
      const targets = downloads.filter((download) => {
        if (!selectedIds.has(download.id)) return false;
        const capabilities = getDownloadCapabilities(download.status);
        return action === 'delete' ? capabilities.remove : capabilities[action];
      });
      if (!targets.length) return;
      await runBatchAction(targets, action);
      clearSelection();
    },
    [downloads, selectedIds, runBatchAction, clearSelection],
  );

  // Read settings on mount for speed limit state
  useEffect(() => {
    api
      .getSettings()
      .then((s) => {
        setSpeedLimited((s.speed_limit_kbps ?? 0) > 0);
        setSavedSpeedLimit(s.speed_limit_kbps ?? 0);
      })
      .catch((e) => showToast('error', api.extractTauriError(e)));
  }, [showToast]);

  const toggleSpeedLimit = useCallback(async () => {
    try {
      const s = await api.getSettings();
      const newLimit = (s.speed_limit_kbps ?? 0) > 0 ? 0 : savedSpeedLimit || 1024;
      await api.saveSettings({ ...s, speed_limit_kbps: newLimit });
      setSpeedLimited(newLimit > 0);
      if (newLimit > 0) setSavedSpeedLimit(newLimit);
      // ponytail: hardcoded "Speed limit:" string replaced with i18n key.
      showToast(
        'info',
        newLimit > 0
          ? t('app.speed_limit_set', { limit: newLimit })
          : t('commandPalette.speed_limit_off'),
      );
    } catch {
      showToast('error', t('settings.save_failed'));
    }
  }, [savedSpeedLimit, showToast, t]);

  // ⌘K opens command palette
  useEffect(() => {
    const handler = (e: KeyboardEvent) => {
      if ((e.metaKey || e.ctrlKey) && e.key === 'k') {
        e.preventDefault();
        setPaletteOpen((v) => !v);
      }
    };
    window.addEventListener('keydown', handler);
    return () => window.removeEventListener('keydown', handler);
  }, []);

  // Drag-and-drop URL zone
  const onDrop = useCallback((e: React.DragEvent) => {
    e.preventDefault();
    setDragging(false);
    const text = e.dataTransfer.getData('text/plain') || e.dataTransfer.getData('text/uri-list');
    if (text && URL_RE.test(text.trim())) {
      setPrefilledUrl(text.trim());
      setIsModalOpen(true);
    }
  }, []);

  const paletteActions = useMemo<PaletteAction[]>(
    () => [
      { id: 'add', label: t('toolbar.add_download'), icon: Plus, run: () => setIsModalOpen(true) },
      { id: 'pause-all', label: t('toolbar.pause_all'), icon: Pause, run: handlePauseAll },
      { id: 'resume-all', label: t('toolbar.resume_all'), icon: Play, run: handleResumeAll },
      {
        id: 'scheduler',
        label: t('toolbar.scheduler'),
        icon: Clock,
        run: () => setSchedulerOpen(true),
      },
      {
        id: 'logs',
        label: t('toolbar.logs'),
        icon: ScrollText,
        run: () => setLogsOpen(true),
      },
      {
        id: 'stats',
        label: t('toolbar.stats'),
        icon: BarChart3,
        run: () => setStatsOpen(true),
      },
      {
        id: 'speed-limit',
        label: speedLimited
          ? t('commandPalette.speed_limit_off')
          : t('commandPalette.speed_limit_on'),
        icon: Gauge,
        run: toggleSpeedLimit,
      },
      {
        id: 'settings',
        label: t('sidebar.settings'),
        icon: Settings,
        run: () => setSettingsOpen(true),
      },
      {
        id: 'theme',
        label: t('commandPalette.toggle_theme'),
        icon: Sun,
        run: () => {
          const isDark = document.documentElement.getAttribute('data-theme') === 'dark';
          const next = isDark ? 'light' : 'dark';
          // ponytail: applyTheme now marks the choice as manual so the OS theme
          // listener no longer overrides it.
          applyTheme(next);
          api
            .getSettings()
            .then((s) => api.saveSettings({ ...s, theme: next }))
            .catch((e) => showToast('error', api.extractTauriError(e)));
        },
      },
      {
        id: 'lang',
        label: t('sidebar.language'),
        icon: Moon,
        run: () => {
          const next = i18n.language === 'en' ? 'tr' : 'en';
          i18n.changeLanguage(next);
          localStorage.setItem('falcon_lang', next);
        },
      },
    ],
    [t, handlePauseAll, handleResumeAll, speedLimited, toggleSpeedLimit, i18n, showToast],
  );

  const toasts = useToastStore((s) => s.toasts);
  const selectedDownloads = useMemo(
    () => downloads.filter((download) => selectedIds.has(download.id)),
    [downloads, selectedIds],
  );
  const canBatchPause = selectedDownloads.some(
    (download) => getDownloadCapabilities(download.status).pause,
  );
  const canBatchResume = selectedDownloads.some(
    (download) => getDownloadCapabilities(download.status).resume,
  );

  return (
    <div
      className="app-shell"
      onDragOver={(e) => {
        e.preventDefault();
        if (!dragging) setDragging(true);
      }}
      onDragLeave={(e) => {
        if (e.relatedTarget === null) setDragging(false);
      }}
      onDrop={onDrop}
    >
      {showOnboarding && (
        <OnboardingWizard
          onComplete={() => setShowOnboarding(false)}
          onSkip={() => setShowOnboarding(false)}
        />
      )}

      <a href="#main-content" className="skip-link">
        {t('app.skip_to_content')}
      </a>
      <TitleBar />

      <div className="app-body">
        <Sidebar
          activeCategory={activeCategory}
          onSelectCategory={setActiveCategory}
          counts={categoryCounts}
        />

        <main className="main-area" id="main-content">
          <Toolbar
            onAddClick={() => setIsModalOpen(true)}
            searchQuery={searchQuery}
            onSearchChange={(value) => {
              setSearchQuery(value);
              void fetchDownloads(undefined, value);
            }}
            onPauseAll={handlePauseAll}
            onResumeAll={handleResumeAll}
            canPauseAll={downloads.some(
              (download) => getDownloadCapabilities(download.status).pause,
            )}
            canResumeAll={downloads.some(
              (download) => getDownloadCapabilities(download.status).resume,
            )}
            clipboardMonitor={clipboardMonitor}
            onToggleClipboard={() => setClipboardMonitor((v) => !v)}
            onOpenSettings={() => setSettingsOpen(true)}
            onOpenScheduler={() => setSchedulerOpen(true)}
            onOpenLogs={() => setLogsOpen(true)}
            onOpenStats={() => setStatsOpen(true)}
            speedLimited={speedLimited}
            onToggleSpeedLimit={toggleSpeedLimit}
          />

          <div className="content-row">
            {selectedIds.size > 1 && (
              <div className="batch-bar" role="toolbar" aria-label={t('app.batch_actions')}>
                <span className="batch-count mono">
                  {selectedIds.size} {t('app.selected')}
                </span>
                <button
                  type="button"
                  className="btn-secondary"
                  onClick={() => handleBatchAction('pause')}
                  disabled={!canBatchPause}
                >
                  <Pause size={14} /> {t('toolbar.pause_all')}
                </button>
                <button
                  type="button"
                  className="btn-secondary"
                  onClick={() => handleBatchAction('resume')}
                  disabled={!canBatchResume}
                >
                  <Play size={14} /> {t('toolbar.resume_all')}
                </button>
                <button
                  type="button"
                  className="btn-secondary"
                  style={{ color: 'var(--danger)' }}
                  onClick={() => setConfirmBatchDelete(true)}
                >
                  <Trash2 size={14} /> {t('downloadItem.delete')}
                </button>
                <button
                  type="button"
                  className="icon-btn"
                  onClick={clearSelection}
                  aria-label={t('app.clear_selection')}
                >
                  <X size={15} />
                </button>
              </div>
            )}
            <ErrorBoundary label="DownloadList">
              <DownloadList
                downloads={downloads}
                category={activeCategory}
                searchQuery={searchQuery}
                selectedId={selectedDownload?.id ?? null}
                onSelectDownload={selectDownload}
                onRefresh={fetchDownloads}
                onRetry={retryFetch}
                loading={loading}
                error={error}
                batchSelectedIds={selectedIds}
              />
            </ErrorBoundary>

            {hasMore && (
              <button
                type="button"
                className="btn-secondary load-more-btn"
                onClick={() => void loadMoreDownloads()}
                disabled={loadingMore}
                aria-busy={loadingMore}
              >
                {loadingMore ? t('app.loading_more') : t('app.load_more')}
              </button>
            )}

            {selectedDownload && (
              <ErrorBoundary label="InspectorPanel">
                <InspectorPanel
                  download={selectedDownload}
                  onClose={() => selectDownload(null)}
                  onRefresh={fetchDownloads}
                />
              </ErrorBoundary>
            )}
          </div>
          <StatusBar downloads={downloads} selected={selectedDownload} />
        </main>
      </div>

      {isModalOpen && (
        <NewDownloadModal
          onClose={() => {
            setIsModalOpen(false);
            setPrefilledUrl('');
          }}
          onSuccess={() => setActiveCategory('All Downloads')}
          onAdded={fetchDownloads}
          initialUrl={prefilledUrl}
        />
      )}

      {toasts.length > 0 && (
        <div className="toast-stack" role="status" aria-live="polite">
          {toasts.map((tt) => (
            <div key={tt.id} className={`toast ${tt.kind}`}>
              {tt.kind === 'success' ? (
                <CheckCircle2 />
              ) : tt.kind === 'error' ? (
                <AlertCircle />
              ) : (
                <Info />
              )}
              <span>{tt.msg}</span>
            </div>
          ))}
        </div>
      )}

      {dragging && (
        <div className="drop-overlay">
          <div className="drop-hint">
            <DownloadCloud />
            <span>{t('app.drop_url')}</span>
          </div>
        </div>
      )}

      {paletteOpen && (
        <CommandPalette
          onClose={() => setPaletteOpen(false)}
          actions={paletteActions}
          onError={(e) => showToast('error', api.extractTauriError(e))}
        />
      )}

      {settingsOpen && <SettingsModal onClose={() => setSettingsOpen(false)} />}
      <SchedulerModal isOpen={schedulerOpen} onClose={() => setSchedulerOpen(false)} />
      {logsOpen && <LogPanel onClose={() => setLogsOpen(false)} />}
      {statsOpen && <StatsPanel onClose={() => setStatsOpen(false)} />}
      {confirmBatchDelete && (
        <ConfirmDialog
          message={t('app.confirm_batch_delete', { count: selectedIds.size })}
          onConfirm={() => {
            setConfirmBatchDelete(false);
            void handleBatchAction('delete');
          }}
          onCancel={() => setConfirmBatchDelete(false)}
        />
      )}
    </div>
  );
}

// ponytail: wrap the whole app in an ErrorBoundary so a top-level render fault
// shows a recovery UI instead of a white screen.
export default function AppRoot() {
  return (
    <ErrorBoundary label="AppRoot">
      <App />
    </ErrorBoundary>
  );
}
