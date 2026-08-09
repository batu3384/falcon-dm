import {} from 'react';
import { useTranslation } from 'react-i18next';
import {
  Plus,
  Play,
  Pause,
  Clock,
  Globe,
  Settings,
  Search,
  Clipboard,
  Gauge,
  ScrollText,
  BarChart3,
} from 'lucide-react';

interface ToolbarProps {
  onAddClick: () => void;
  searchQuery: string;
  onSearchChange: (q: string) => void;
  onPauseAll: () => void;
  onResumeAll: () => void;
  canPauseAll?: boolean;
  canResumeAll?: boolean;
  clipboardMonitor?: boolean;
  onToggleClipboard?: () => void;
  onOpenSettings: () => void;
  onOpenScheduler: () => void;
  onOpenLogs?: () => void;
  onOpenStats?: () => void;
  speedLimited?: boolean;
  onToggleSpeedLimit?: () => void;
}

export default function Toolbar({
  onAddClick,
  searchQuery,
  onSearchChange,
  onPauseAll,
  onResumeAll,
  canPauseAll = true,
  canResumeAll = true,
  clipboardMonitor,
  onToggleClipboard,
  onOpenSettings,
  onOpenScheduler,
  onOpenLogs,
  onOpenStats,
  speedLimited,
  onToggleSpeedLimit,
}: ToolbarProps) {
  const { t, i18n } = useTranslation();
  const isMac = navigator.platform.toUpperCase().includes('MAC');

  const toggleLanguage = () => {
    const next = i18n.language === 'en' ? 'tr' : 'en';
    i18n.changeLanguage(next);
    localStorage.setItem('falcon_lang', next);
    document.documentElement.lang = next;
  };

  return (
    <header className="toolbar no-drag">
      <div className="toolbar-row">
        <div className="toolbar-actions">
          <button
            type="button"
            className="btn-primary"
            onClick={onAddClick}
            aria-label={t('toolbar.add_download')}
          >
            <Plus size={15} strokeWidth={2.5} />
            <span>{t('toolbar.add_download')}</span>
          </button>

          <div className="toolbar-divider" />

          <button
            type="button"
            className="icon-btn"
            onClick={onResumeAll}
            disabled={!canResumeAll}
            title={t('toolbar.resume_all')}
            aria-label={t('toolbar.resume_all')}
          >
            <Play size={15} />
          </button>
          <button
            type="button"
            className="icon-btn"
            onClick={onPauseAll}
            disabled={!canPauseAll}
            title={t('toolbar.pause_all')}
            aria-label={t('toolbar.pause_all')}
          >
            <Pause size={15} />
          </button>
          <button
            type="button"
            className="icon-btn"
            onClick={onOpenScheduler}
            title={t('toolbar.scheduler')}
            aria-label={t('toolbar.scheduler')}
          >
            <Clock size={15} />
          </button>
          {onToggleSpeedLimit && (
            <button
              type="button"
              className={`icon-btn ${speedLimited ? 'active' : ''}`}
              onClick={onToggleSpeedLimit}
              title={t('toolbar.speed_limit')}
              aria-label={t('toolbar.speed_limit')}
              aria-pressed={speedLimited}
            >
              <Gauge size={15} />
            </button>
          )}
          {onToggleClipboard && (
            <button
              type="button"
              className={`icon-btn ${clipboardMonitor ? 'active' : ''}`}
              onClick={onToggleClipboard}
              title={t('toolbar.clipboard')}
              aria-label={t('toolbar.clipboard')}
              aria-pressed={clipboardMonitor}
            >
              <Clipboard size={15} />
            </button>
          )}
        </div>

        <div className="toolbar-actions">
          <div className="search-wrap">
            <Search size={14} />
            <input
              className="search-input"
              placeholder={t('toolbar.search_placeholder')}
              value={searchQuery}
              onChange={(e) => onSearchChange(e.target.value)}
              aria-label={t('toolbar.search_placeholder')}
            />
            <kbd className="kbd">{isMac ? '⌘K' : 'Ctrl+K'}</kbd>
          </div>
          <button
            type="button"
            className="icon-btn lang-btn"
            onClick={toggleLanguage}
            title={t('sidebar.language')}
            aria-label={t('sidebar.language')}
          >
            <Globe size={15} />
            <span className="lang-tag">{i18n.language.toUpperCase()}</span>
          </button>

          <div className="toolbar-divider" />

          {onOpenLogs && (
            <button
              type="button"
              className="icon-btn"
              onClick={onOpenLogs}
              title={t('toolbar.logs')}
              aria-label={t('toolbar.logs')}
            >
              <ScrollText size={15} />
            </button>
          )}
          {onOpenStats && (
            <button
              type="button"
              className="icon-btn"
              onClick={onOpenStats}
              title={t('toolbar.stats')}
              aria-label={t('toolbar.stats')}
            >
              <BarChart3 size={15} />
            </button>
          )}
          <button
            type="button"
            className="icon-btn"
            onClick={onOpenSettings}
            title={t('sidebar.settings')}
            aria-label={t('sidebar.settings')}
          >
            <Settings size={15} />
          </button>
        </div>
      </div>
    </header>
  );
}
