import { useEffect, useRef, useState } from 'react';
import { useTranslation } from 'react-i18next';
import { Gauge, PanelRight, Pause, Play, Plus, Search, X } from 'lucide-react';
import { useDownloadsStore, totalLiveSpeed } from '../store/downloads';
import { useAppStore } from '../store/app';
import { useUiStore } from '../store/ui';
import { formatSpeed, loadedCountLabel } from '../types';

interface ToolbarProps {
  onPauseAll: () => void;
  onResumeAll: () => void;
  onToggleSpeedLimit: () => void;
}

export default function Toolbar({ onPauseAll, onResumeAll, onToggleSpeedLimit }: ToolbarProps) {
  const { t, i18n } = useTranslation();
  const view = useDownloadsStore((s) => s.view);
  const rows = useDownloadsStore((s) => s.rows);
  const hasMore = useDownloadsStore((s) => s.hasMore);
  const storeSearch = useDownloadsStore((s) => s.search);
  const setSearch = useDownloadsStore((s) => s.setSearch);
  const liveSpeeds = useDownloadsStore((s) => s.liveSpeeds);
  const stats = useAppStore((s) => s.stats);
  const inspectorVisible = useUiStore((s) => s.inspectorVisible);
  const toggleInspector = useUiStore((s) => s.toggleInspector);
  const openNewDownload = useUiStore((s) => s.openNewDownload);
  const [query, setQuery] = useState(storeSearch);
  const inputRef = useRef<HTMLInputElement>(null);

  // Debounced server-side search.
  useEffect(() => {
    const timer = window.setTimeout(() => void setSearch(query), 250);
    return () => window.clearTimeout(timer);
  }, [query, setSearch]);

  // ⌘F focuses the field.
  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      if ((e.metaKey || e.ctrlKey) && e.key.toLowerCase() === 'f') {
        if (document.querySelector('.sheet-overlay')) return;
        e.preventDefault();
        inputRef.current?.focus();
        inputRef.current?.select();
      }
    };
    window.addEventListener('keydown', onKey);
    return () => window.removeEventListener('keydown', onKey);
  }, []);

  const active = stats ? stats.active : 0;
  const speed = totalLiveSpeed(liveSpeeds);
  const count = loadedCountLabel(rows.length, hasMore);
  const subtitle =
    active > 0
      ? t('toolbar.subtitle_active', { count: active, speed: formatSpeed(speed, i18n.language) })
      : t('toolbar.subtitle_items', { count: rows.length, label: count });
  const canPause = !!stats && stats.active + stats.queued > 0;
  const canResume = !!stats && stats.paused + stats.failed > 0;
  const limited = (stats?.speed_limit_kbps ?? 0) > 0;

  return (
    <header className="toolbar" data-tauri-drag-region>
      <div className="toolbar-title" data-tauri-drag-region>
        <h1 data-tauri-drag-region>{t(`views.${view}`)}</h1>
        <span className="toolbar-subtitle" data-tauri-drag-region>
          {subtitle}
        </span>
      </div>

      <div className="toolbar-group">
        <button
          type="button"
          className="tool-btn"
          onClick={onResumeAll}
          disabled={!canResume}
          title={t('toolbar.resume_all')}
          aria-label={t('toolbar.resume_all')}
        >
          <Play size={16} aria-hidden={true} />
        </button>
        <button
          type="button"
          className="tool-btn"
          onClick={onPauseAll}
          disabled={!canPause}
          title={t('toolbar.pause_all')}
          aria-label={t('toolbar.pause_all')}
        >
          <Pause size={16} aria-hidden={true} />
        </button>
        <span className="toolbar-sep" aria-hidden="true" />
        <button
          type="button"
          className="tool-btn"
          onClick={onToggleSpeedLimit}
          aria-pressed={limited}
          title={
            limited
              ? t('toolbar.speed_limit_on', {
                  limit: formatSpeed((stats?.speed_limit_kbps ?? 0) * 1000, i18n.language),
                })
              : t('toolbar.speed_limit_off')
          }
          aria-label={t('toolbar.speed_limit')}
        >
          <Gauge size={16} aria-hidden={true} />
        </button>
        <button
          type="button"
          className="tool-btn"
          onClick={toggleInspector}
          aria-pressed={inspectorVisible}
          title={t('toolbar.inspector')}
          aria-label={t('toolbar.inspector')}
        >
          <PanelRight size={16} aria-hidden={true} />
        </button>
      </div>

      <div className="search" role="search">
        <Search size={13} aria-hidden={true} />
        <input
          ref={inputRef}
          type="search"
          placeholder={t('toolbar.search')}
          aria-label={t('toolbar.search')}
          value={query}
          onChange={(e) => setQuery(e.target.value)}
          onKeyDown={(e) => {
            if (e.key === 'Escape' && query) {
              e.stopPropagation();
              setQuery('');
            }
          }}
          spellCheck={false}
          autoComplete="off"
        />
        {query && (
          <button
            type="button"
            className="search-clear"
            onClick={() => {
              setQuery('');
              inputRef.current?.focus();
            }}
            aria-label={t('toolbar.clear_search')}
          >
            <X size={11} strokeWidth={3} aria-hidden={true} />
          </button>
        )}
      </div>

      <button
        type="button"
        className="tool-btn accent"
        onClick={() => openNewDownload()}
        title={`${t('toolbar.new_download')} (⌘N)`}
        aria-label={t('toolbar.new_download')}
      >
        <Plus size={18} strokeWidth={2} aria-hidden={true} />
      </button>
    </header>
  );
}
