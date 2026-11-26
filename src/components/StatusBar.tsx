import { useTranslation } from 'react-i18next';
import { ArrowDown, Clock, Gauge, HardDrive } from 'lucide-react';
import { useDownloadsStore, totalLiveSpeed } from '../store/downloads';
import { useAppStore } from '../store/app';
import { useUiStore } from '../store/ui';
import { formatBytes, formatSpeed } from '../types';

/** Below this the volume is about to fill up. */
const LOW_DISK_BYTES = 2 * 1024 ** 3;

export default function StatusBar() {
  const { t, i18n } = useTranslation();
  const liveSpeeds = useDownloadsStore((s) => s.liveSpeeds);
  const stats = useAppStore((s) => s.stats);
  const schedule = useAppStore((s) => s.schedule);
  const openSettings = useUiStore((s) => s.openSettings);
  const lang = i18n.language;
  const speed = totalLiveSpeed(liveSpeeds);
  const limit = stats?.speed_limit_kbps ?? 0;
  const free = stats?.free_disk_bytes ?? null;

  return (
    <footer className="statusbar" role="contentinfo">
      <span className="status-item" title={t('status_bar.total_speed')}>
        <ArrowDown size={12} aria-hidden={true} />
        <strong>{formatSpeed(speed, lang)}</strong>
      </span>
      {stats && stats.active + stats.queued > 0 && (
        <span className="status-item">
          {t('status_bar.summary', { active: stats.active, queued: stats.queued })}
        </span>
      )}
      <span className="spacer" />
      {limit > 0 && (
        <button
          type="button"
          className="status-link"
          onClick={() => openSettings('downloads')}
          title={t('status_bar.edit_limit')}
        >
          <Gauge size={12} aria-hidden={true} />
          {t('status_bar.limit', { limit: formatSpeed(limit * 1024, lang) })}
        </button>
      )}
      {schedule?.active && schedule.start_time && schedule.stop_time && (
        <button
          type="button"
          className="status-link"
          onClick={() => openSettings('downloads')}
          title={t('status_bar.edit_schedule')}
        >
          <Clock size={12} aria-hidden={true} />
          {t('status_bar.schedule', { start: schedule.start_time, stop: schedule.stop_time })}
        </button>
      )}
      {free !== null && (
        <span
          className={`status-item${free < LOW_DISK_BYTES ? ' low' : ''}`}
          title={t('status_bar.free_space_hint')}
        >
          <HardDrive size={12} aria-hidden={true} />
          {t('status_bar.free_space', { size: formatBytes(free, lang) })}
        </span>
      )}
    </footer>
  );
}
