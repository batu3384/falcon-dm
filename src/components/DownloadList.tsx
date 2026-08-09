import { useMemo, useRef } from 'react';
import { useTranslation } from 'react-i18next';
import { AlertCircle, Inbox } from 'lucide-react';
import { useVirtualizer } from '@tanstack/react-virtual';
import DownloadItem from './DownloadItem';
import type { DownloadModel } from '../types';

interface DownloadListProps {
  downloads: DownloadModel[];
  category: string;
  searchQuery: string;
  selectedId: number | null;
  onSelectDownload: (d: DownloadModel | null, mods?: { meta?: boolean; shift?: boolean }) => void;
  onRefresh: () => void;
  onRetry?: () => void;
  onAddClick?: () => void;
  loading?: boolean;
  error?: string | null;
  batchSelectedIds?: Set<number>;
}

// ponytail: item height is fixed by the .dl-item layout. Used as the virtualizer
// estimate; the actual rendered height matches so there's no drift.
const ITEM_HEIGHT = 64;

export default function DownloadList({
  downloads,
  category,
  searchQuery,
  selectedId,
  onSelectDownload,
  onRefresh,
  onRetry,
  onAddClick,
  loading = false,
  error = null,
  batchSelectedIds,
}: DownloadListProps) {
  const { t } = useTranslation();
  const scrollRef = useRef<HTMLDivElement>(null);

  const filtered = useMemo(() => {
    const q = searchQuery.trim().toLowerCase();
    return downloads.filter((d) => {
      if (q && !d.filename.toLowerCase().includes(q) && !d.url.toLowerCase().includes(q))
        return false;
      if (category === 'All Downloads') return true;
      if (category === 'Downloading') return d.status === 'Downloading' || d.status === 'Merging';
      if (category === 'Completed') return d.status === 'Completed';
      if (category === 'Paused') return d.status === 'Paused' || d.status === 'Queued';
      if (category === 'Failed') return d.status === 'Failed';
      if (category === 'Archived') return true; // archived rows fetched separately
      if (category === 'Video') return d.category === 'Video';
      if (category === 'Music') return d.category === 'Music';
      if (category === 'Documents') return d.category === 'Document';
      if (category === 'Compressed') return d.category === 'Archive';
      if (category === 'Programs') return d.category === 'Program';
      return true;
    });
  }, [downloads, category, searchQuery]);

  // ponytail: virtualize the list. Previously all 500 items rendered as DOM
  // nodes; with progress ticks firing several times a second this caused severe
  // jank. The virtualizer renders only the visible window (~10–15 items).
  const virtualizer = useVirtualizer({
    count: filtered.length,
    getScrollElement: () => scrollRef.current,
    estimateSize: () => ITEM_HEIGHT,
    overscan: 8,
  });

  const titleMap: Record<string, string> = {
    'All Downloads': t('sidebar.all_downloads'),
    Downloading: t('sidebar.active'),
    Paused: t('sidebar.paused'),
    Failed: t('sidebar.failed'),
    Archived: t('sidebar.archived'),
    Completed: t('sidebar.completed'),
    Video: t('sidebar.videos'),
    Music: t('sidebar.music'),
    Documents: t('sidebar.documents'),
    Compressed: t('sidebar.compressed'),
    Programs: t('sidebar.programs'),
  };

  const activeCount = downloads.filter((d) => d.status === 'Downloading').length;
  const totalHeight = virtualizer.getTotalSize();
  const items = virtualizer.getVirtualItems();

  return (
    <div className="download-view">
      <div className="view-head">
        <h2 className="view-title">{titleMap[category] || category}</h2>
        <span className="view-count">{filtered.length}</span>
        {activeCount > 0 && (
          <span className="live-pill">
            <span className="pulse-dot" />
            {activeCount} {t('sidebar.active').toLowerCase()}
          </span>
        )}
      </div>

      <div className="dl-list" role="list" ref={scrollRef} style={{ overflowY: 'auto', flex: 1 }}>
        {error && downloads.length > 0 && (
          <div
            role="alert"
            style={{
              alignItems: 'center',
              display: 'flex',
              gap: 8,
              justifyContent: 'space-between',
              margin: '8px 12px',
              padding: '8px 10px',
            }}
          >
            <span title={error}>{t('downloadList.load_error_desc')}</span>
            <button type="button" className="btn-secondary" onClick={onRetry ?? onRefresh}>
              {t('downloadList.retry')}
            </button>
          </div>
        )}
        {loading ? (
          // ponytail: shimmer skeletons mirror the real .dl-item geometry so the
          // first paint reads as "loading" instead of a frozen spinner, and the
          // layout doesn't jump when real rows arrive.
          <div className="skeleton-list" aria-label={t('downloadList.loading')} role="status">
            {[0, 1, 2, 3, 4].map((i) => (
              <div className="skeleton-row" key={i}>
                <div className="skeleton-thumb shimmer" />
                <div className="skeleton-body">
                  <div className="skeleton-bar w-60 shimmer" />
                  <div className="skeleton-bar w-100 shimmer" />
                  <div className="skeleton-bar w-35 shimmer" />
                </div>
                <div className="skeleton-actions shimmer" />
              </div>
            ))}
          </div>
        ) : error && downloads.length === 0 ? (
          <div className="empty-state error-state" role="alert">
            <div className="empty-icon">
              <AlertCircle strokeWidth={1.5} />
            </div>
            <div className="empty-title">{t('downloadList.load_error')}</div>
            <div className="empty-desc" title={error}>
              {t('downloadList.load_error_desc')}
            </div>
            <button type="button" className="btn-primary" onClick={onRetry ?? onRefresh}>
              {t('downloadList.retry')}
            </button>
          </div>
        ) : filtered.length === 0 ? (
          <div className="empty-state">
            <div className="empty-icon">
              <Inbox strokeWidth={1.5} />
            </div>
            <div className="empty-title">
              {searchQuery ? t('downloadList.no_search_results') : t('downloadList.no_downloads')}
            </div>
            <div className="empty-desc">
              {searchQuery ? t('downloadList.no_search_desc') : t('downloadList.no_downloads_desc')}
            </div>
            {!searchQuery && onAddClick && (
              <button
                type="button"
                className="btn-primary"
                style={{ marginTop: 12 }}
                onClick={onAddClick}
              >
                {t('downloadList.add_cta')}
              </button>
            )}
          </div>
        ) : (
          <div style={{ height: totalHeight, position: 'relative', width: '100%' }}>
            {items.map((virtualItem) => {
              const dl = filtered[virtualItem.index];
              if (!dl) return null;
              return (
                <div
                  key={dl.id}
                  data-index={virtualItem.index}
                  ref={virtualizer.measureElement}
                  style={{
                    position: 'absolute',
                    top: 0,
                    left: 0,
                    width: '100%',
                    transform: `translateY(${virtualItem.start}px)`,
                  }}
                >
                  <DownloadItem
                    item={dl}
                    isSelected={selectedId === dl.id}
                    isBatchSelected={batchSelectedIds?.has(dl.id) ?? false}
                    onSelect={onSelectDownload}
                    onRefresh={onRefresh}
                  />
                </div>
              );
            })}
          </div>
        )}
      </div>
    </div>
  );
}
