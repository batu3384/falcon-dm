import { useTranslation } from 'react-i18next';
import {
  AlertTriangle,
  AppWindow,
  Archive,
  CheckCircle2,
  Download,
  FileArchive,
  FileText,
  Film,
  Layers,
  Music,
  Pause,
  Settings,
} from 'lucide-react';
import { useDownloadsStore } from '../store/downloads';
import { useAppStore } from '../store/app';
import { useUiStore } from '../store/ui';
import { CATEGORY_VIEWS, LIBRARY_VIEWS, viewCounts, type ViewId } from '../lib/downloadView';

const ICONS: Record<ViewId, typeof Download> = {
  all: Layers,
  active: Download,
  paused: Pause,
  completed: CheckCircle2,
  failed: AlertTriangle,
  archived: Archive,
  video: Film,
  music: Music,
  document: FileText,
  archive: FileArchive,
  program: AppWindow,
};

export default function Sidebar() {
  const { t } = useTranslation();
  const view = useDownloadsStore((s) => s.view);
  const setView = useDownloadsStore((s) => s.setView);
  const stats = useAppStore((s) => s.stats);
  const extension = useAppStore((s) => s.extension);
  const openSettings = useUiStore((s) => s.openSettings);
  const counts = stats ? viewCounts(stats) : null;
  const paired = (extension?.approved_extension_ids.length ?? 0) > 0;
  const pending = (extension?.pending.length ?? 0) > 0;

  const item = (id: ViewId) => {
    const Icon = ICONS[id];
    const count = counts?.[id] ?? 0;
    return (
      <li key={id}>
        <button
          type="button"
          className={`nav-item${id === 'failed' && count > 0 ? ' attention' : ''}`}
          aria-current={view === id ? 'page' : undefined}
          onClick={() => void setView(id)}
        >
          <Icon size={16} strokeWidth={1.8} aria-hidden={true} />
          <span className="nav-label">{t(`views.${id}`)}</span>
          {count > 0 && <span className="nav-count">{count}</span>}
        </button>
      </li>
    );
  };

  return (
    <aside className="sidebar" aria-label={t('sidebar.label')}>
      <div className="sidebar-top" data-tauri-drag-region />
      <nav className="sidebar-scroll" aria-label={t('sidebar.label')}>
        <section className="sidebar-section">
          <h2 className="sidebar-heading">{t('sidebar.library')}</h2>
          <ul className="nav-list">{LIBRARY_VIEWS.map(item)}</ul>
        </section>
        <section className="sidebar-section">
          <h2 className="sidebar-heading">{t('sidebar.kinds')}</h2>
          <ul className="nav-list">{CATEGORY_VIEWS.map(item)}</ul>
        </section>
      </nav>
      <div className="sidebar-foot">
        <button
          type="button"
          className="ext-status"
          onClick={() => openSettings('extension')}
          title={t('sidebar.extension_settings')}
        >
          <span className={`dot${pending ? ' wait' : paired ? ' on' : ''}`} aria-hidden="true" />
          <span>
            {pending
              ? t('sidebar.extension_pending')
              : paired
                ? t('sidebar.extension_on')
                : t('sidebar.extension_off')}
          </span>
        </button>
        <button
          type="button"
          className="icon-btn"
          onClick={() => openSettings()}
          title={`${t('settings.title')} (⌘,)`}
          aria-label={t('settings.title')}
        >
          <Settings size={16} aria-hidden={true} />
        </button>
      </div>
    </aside>
  );
}
