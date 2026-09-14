import { useMemo, useRef } from 'react';
import { useTranslation } from 'react-i18next';
import { AlertCircle, AlertTriangle, Inbox, SearchX } from 'lucide-react';
import { useVirtualizer, defaultRangeExtractor } from '@tanstack/react-virtual';
import DownloadItem from './DownloadItem';
import { downloadMatchesView } from '../lib/downloadView';
import type { DownloadModel } from '../types';
import { loadedCountLabel } from '../types';

interface DownloadListProps {
  downloads: DownloadModel[];
  category: string;
  searchQuery: string;
  selectedId: number | null;
  onSelectDownload: (
    d: DownloadModel | null,
    mods?: { meta?: boolean; shift?: boolean },
    visibleIds?: number[],
  ) => void;
  onRefresh: () => void;
  onRetry?: () => void;
  onAddClick?: () => void;
  loading?: boolean;
  error?: string | null;
  batchSelectedIds?: Set<number>;
  hasMore?: boolean;
  onLoadMore?: () => void;
}

// ponytail: item height is fixed by the .dl-item layout. Used as the virtualizer
// estimate; the actual rendered height matches so there's no drift.
const ITEM_HEIGHT = 68;

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
  hasMore = false,
  onLoadMore,
}: DownloadListProps) {
  const { t } = useTranslation();
  const scrollRef = useRef<HTMLDivElement>(null);

  const filtered = useMemo(() => {
    const q = searchQuery.trim().toLowerCase();
    return downloads.filter((d) => {
      if (q && !d.filename.toLowerCase().includes(q) && !d.url.toLowerCase().includes(q))
        return false;
      return downloadMatchesView(d, category, category === 'Archived');
    });
  }, [downloads, category, searchQuery]);

  // ponytail: virtualize the list. Previously all 500 items rendered as DOM
  // nodes; with progress ticks firing several times a second this caused severe
  // jank. The virtualizer renders only the visible window (~10–15 items).
  const selectedIndex = filtered.findIndex((d) => d.id === selectedId);
  const virtualizer = useVirtualizer({
    count: filtered.length,
    getScrollElement: () => scrollRef.current,
    estimateSize: () => ITEM_HEIGHT,
    overscan: 8,
    rangeExtractor: (range) => {
      const indexes = defaultRangeExtractor(range);
      if (
        selectedIndex >= 0 &&
        selectedIndex < range.count &&
        !indexes.includes(selectedIndex)
      ) {
        return [...indexes, selectedIndex].sort((a, b) => a - b);
      }
      return indexes;
    },
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

  const listInteractive = !loading && filtered.length > 0;
  const visibleIds = useMemo(() => filtered.map((d) => d.id), [filtered]);
  const activeCount = filtered.filter(
    (d) => d.status === 'Downloading' || d.status === 'Merging',
  ).length;
  const totalHeight = virtualizer.getTotalSize();
  const items = virtualizer.getVirtualItems();

  const onListKeyDown = (e: React.KeyboardEvent) => {
    if (!listInteractive) return;
    const fromField = (e.target as HTMLElement | null)?.closest('button, input, textarea, select');
    if (fromField && fromField !== e.currentTarget) return;
    if (e.key === 'Enter' || e.key === ' ') {
      // Row handler toggles select; bubbling would re-select immediately.
      if ((e.target as HTMLElement | null)?.closest('[role="listitem"]')) return;
      e.preventDefault();
      const item = selectedIndex >= 0 ? filtered[selectedIndex] : filtered[0];
      if (item) onSelectDownload(item);
      return;
    }
    if (e.key !== 'ArrowDown' && e.key !== 'ArrowUp' && e.key !== 'Home' && e.key !== 'End') {
      return;
    }
    e.preventDefault();
    const last = filtered.length - 1;
    let next = selectedIndex < 0 ? 0 : selectedIndex;
    if (e.key === 'ArrowDown') {
      if (next >= last && hasMore) onLoadMore?.();
      next = Math.min(next + 1, last);
    } else if (e.key === 'ArrowUp') next = Math.max(next - 1, 0);
    else if (e.key === 'Home') next = 0;
    else next = last;
    const item = filtered[next];
    if (!item) return;
    onSelectDownload(item);
    virtualizer.scrollToIndex(next);
  };

  return (
    <div className="download-view">
      <div className="view-head">
        <h2 className="view-title">{titleMap[category] || category}</h2>
        <span className="view-count">{loadedCountLabel(filtered.length, hasMore)}</span>
        {activeCount > 0 && (
          <span className="live-pill" aria-live="polite">
            <span className="pulse-dot" />
            {activeCount} {t('sidebar.active').toLowerCase()}
          </span>
        )}
      </div>

      {error && downloads.length > 0 && (
        <div className="diagnostic-error" role="alert">
          <span title={error}>{t('downloadList.load_error_desc')}</span>
          <button type="button" className="btn-secondary" onClick={onRetry ?? onRefresh}>
            {t('downloadList.retry')}
          </button>
        </div>
      )}
      <div
        className="dl-list"
        role={listInteractive ? 'list' : undefined}
        aria-label={titleMap[category] || category}
        tabIndex={listInteractive ? 0 : undefined}
        onKeyDown={onListKeyDown}
        ref={scrollRef}
      >
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
            <div className="empty-icon" aria-hidden="true">
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
            <div className="empty-icon" aria-hidden="true">
              {searchQuery ? (
                <SearchX strokeWidth={1.5} />
              ) : category === 'Failed' ? (
                <AlertTriangle strokeWidth={1.5} />
              ) : (
                <Inbox strokeWidth={1.5} />
              )}
            </div>
            <div className="empty-title">
              {searchQuery
                ? t('downloadList.no_search_results')
                : category === 'Failed'
                  ? t('downloadList.empty_failed')
                  : t('downloadList.no_downloads')}
            </div>
            <div className="empty-desc">
              {searchQuery
                ? t('downloadList.no_search_desc')
                : category === 'Failed'
                  ? t('downloadList.empty_failed_desc')
                  : t('downloadList.no_downloads_desc')}
            </div>
            {!searchQuery && category !== 'Failed' && onAddClick && (
              <button type="button" className="btn-primary empty-cta" onClick={onAddClick}>
                {t('downloadList.add_cta')}
              </button>
            )}
          </div>
        ) : (
          <div className="dl-virtual-spacer" style={{ height: totalHeight }} role="presentation">
            {items.map((virtualItem) => {
              const dl = filtered[virtualItem.index];
              if (!dl) return null;
              return (
                <div
                  key={dl.id}
                  id={`dl-row-${dl.id}`}
                  role="listitem"
                  aria-current={selectedId === dl.id ? 'true' : undefined}
                  className="dl-virtual-row"
                  data-index={virtualItem.index}
                  ref={virtualizer.measureElement}
                  style={{ transform: `translateY(${virtualItem.start}px)` }}
                >
                  <DownloadItem
                    item={dl}
                    isSelected={selectedId === dl.id}
                    isBatchSelected={batchSelectedIds?.has(dl.id) ?? false}
                    onSelect={(item, mods) => onSelectDownload(item, mods, visibleIds)}
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
