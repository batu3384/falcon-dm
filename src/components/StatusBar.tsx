import { useMemo } from 'react';
import { useTranslation } from 'react-i18next';
import type { DownloadModel } from '../types';
import { formatBytes, loadedCountLabel } from '../types';
import SpeedGraph from './SpeedGraph';

interface StatusBarProps {
  downloads: DownloadModel[];
  selected: DownloadModel | null;
  hasMore?: boolean;
}

export default function StatusBar({ downloads, selected, hasMore = false }: StatusBarProps) {
  const { t } = useTranslation();
  // ponytail: memoize the 4 filter/reduce passes that previously ran on every
  // progress tick (several times a second) regardless of whether counts changed.
  const { activeCount, queuedCount, totalSpeed } = useMemo(() => {
    let active = 0;
    let queued = 0;
    let speed = 0;
    for (const d of downloads) {
      if (d.status === 'Downloading' || d.status === 'Merging') {
        active++;
        speed += d.speed || 0;
      } else if (d.status === 'Queued') {
        queued++;
      }
    }
    return {
      activeCount: active,
      queuedCount: queued,
      totalSpeed: speed,
    };
  }, [downloads]);
  const listCount = loadedCountLabel(downloads.length, hasMore);

  return (
    <footer className="status-bar no-drag" role="contentinfo">
      <span className="sr-only" role="status" aria-live="polite">
        {activeCount} {t('statusBar.active')}, {queuedCount} {t('statusBar.queued')}, {listCount}{' '}
        {t('statusBar.total')}
      </span>
      <span>
        {t('statusBar.speed')}: <strong className="mono">{formatBytes(totalSpeed)}/s</strong>
      </span>
      {activeCount > 0 && <SpeedGraph speed={totalSpeed} />}
      <span className="status-sep" aria-hidden />
      <span>
        {t('statusBar.active')}: <strong>{activeCount}</strong>
      </span>
      <span className="status-sep" aria-hidden />
      <span>
        {t('statusBar.queued')}: <strong>{queuedCount}</strong>
      </span>
      <span className="status-sep" aria-hidden />
      <span>
        {t('statusBar.total')}: <strong>{listCount}</strong>
      </span>
      {selected && (
        <>
          <span className="status-sep" aria-hidden />
          <span className="status-selected" title={selected.filename}>
            {selected.filename}
          </span>
        </>
      )}
    </footer>
  );
}
