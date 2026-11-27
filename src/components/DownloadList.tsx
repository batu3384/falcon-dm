import { useCallback, useEffect, useMemo, useRef, useState } from 'react';
import { useTranslation } from 'react-i18next';
import {
  AlertTriangle,
  Archive,
  CheckCircle2,
  Download,
  Film,
  FileArchive,
  FileText,
  AppWindow,
  Music,
  Pause,
  SearchX,
  Inbox,
} from 'lucide-react';
import { useVirtualizer } from '@tanstack/react-virtual';
import { queuePositions, useDownloadsStore } from '../store/downloads';
import { useUiStore } from '../store/ui';
import type { DownloadModel } from '../types';
import type { ViewId } from '../lib/downloadView';
import { getDownloadCapabilities } from '../lib/downloadCapabilities';
import { useDownloadActions } from '../lib/useDownloadActions';
import { downloadRowShortcut } from '../lib/rowShortcuts';
import { DownloadRow } from './DownloadRow';
import { ContextMenu } from './ContextMenu';
import { buildDownloadMenu } from './downloadMenu';

const ROW_HEIGHT = 56;

const EMPTY_ICON: Record<ViewId, typeof Inbox> = {
  all: Download,
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

function isEditable(target: EventTarget | null): boolean {
  return !!(target as HTMLElement | null)?.closest('input, textarea, select, [contenteditable]');
}

export default function DownloadList() {
  const { t } = useTranslation();
  const rows = useDownloadsStore((s) => s.rows);
  const view = useDownloadsStore((s) => s.view);
  const search = useDownloadsStore((s) => s.search);
  const loading = useDownloadsStore((s) => s.loading);
  const error = useDownloadsStore((s) => s.error);
  const hasMore = useDownloadsStore((s) => s.hasMore);
  const loadingMore = useDownloadsStore((s) => s.loadingMore);
  const selectedIds = useDownloadsStore((s) => s.selectedIds);
  const primaryId = useDownloadsStore((s) => s.primaryId);
  const select = useDownloadsStore((s) => s.select);
  const selectAll = useDownloadsStore((s) => s.selectAll);
  const refresh = useDownloadsStore((s) => s.refresh);
  const loadMore = useDownloadsStore((s) => s.loadMore);
  const openNewDownload = useUiStore((s) => s.openNewDownload);
  const actions = useDownloadActions();
  const scrollRef = useRef<HTMLDivElement>(null);
  const [menu, setMenu] = useState<{ rows: DownloadModel[]; x: number; y: number } | null>(null);

  // Retry countdowns tick once a second, only while one is visible.
  const [now, setNow] = useState(() => Date.now());
  const hasRetry = rows.some((row) => row.status === 'Queued' && row.retry_at != null);
  useEffect(() => {
    if (!hasRetry) return;
    const timer = window.setInterval(() => setNow(Date.now()), 1000);
    return () => window.clearInterval(timer);
  }, [hasRetry]);

  const virtualizer = useVirtualizer({
    count: rows.length,
    getScrollElement: () => scrollRef.current,
    estimateSize: () => ROW_HEIGHT,
    overscan: 10,
  });
  const items = virtualizer.getVirtualItems();

  // Fetch the next page when the end of the list scrolls into view.
  const lastIndex = items[items.length - 1]?.index ?? -1;
  useEffect(() => {
    if (hasMore && !loadingMore && lastIndex >= rows.length - 5) void loadMore();
  }, [hasMore, loadingMore, lastIndex, rows.length, loadMore]);

  const selectedRows = useMemo(
    () => rows.filter((row) => selectedIds.has(row.id)),
    [rows, selectedIds],
  );
  const positions = useMemo(() => queuePositions(rows), [rows]);

  const openMenu = useCallback(
    (row: DownloadModel, x: number, y: number) => {
      const current = useDownloadsStore.getState().selectedIds;
      if (!current.has(row.id)) select(row.id);
      const ids = current.has(row.id) ? current : new Set([row.id]);
      setMenu({ rows: useDownloadsStore.getState().rows.filter((r) => ids.has(r.id)), x, y });
    },
    [select],
  );

  const onToggle = useCallback((row: DownloadModel) => void actions.toggle([row]), [actions]);
  const onReveal = useCallback((row: DownloadModel) => void actions.reveal(row), [actions]);

  // Moving end of a keyboard range; the store's primaryId is the fixed anchor.
  const cursorRef = useRef<number | null>(null);
  const moveSelection = (index: number, extend: boolean) => {
    const target = Math.max(0, Math.min(rows.length - 1, index));
    const row = rows[target];
    if (!row) return;
    select(row.id, extend ? { range: true } : undefined);
    cursorRef.current = row.id;
    virtualizer.scrollToIndex(target);
  };

  const openMenuForSelection = () => {
    if (!selectedRows.length) return;
    const row =
      rows.find((candidate) => candidate.id === primaryId) ?? selectedRows[selectedRows.length - 1];
    const el = document.getElementById(`dl-row-${row.id}`);
    const rect = el?.getBoundingClientRect();
    setMenu({ rows: selectedRows, x: (rect?.left ?? 40) + 40, y: rect?.bottom ?? 80 });
  };

  const onKeyDown = (e: React.KeyboardEvent) => {
    if (isEditable(e.target) || rows.length === 0) return;
    const shortcut = downloadRowShortcut(e);
    if (shortcut && selectedRows.length) {
      if (shortcut === 'menu') {
        e.preventDefault();
        openMenuForSelection();
        return;
      }
      const row = selectedRows.length === 1 ? selectedRows[0] : null;
      if (shortcut === 'reveal' && row) {
        e.preventDefault();
        void actions.reveal(row);
        return;
      }
      if (shortcut === 'open' && row && getDownloadCapabilities(row.status).open) {
        e.preventDefault();
        void actions.open(row);
        return;
      }
    }
    const cursorId = e.shiftKey && cursorRef.current !== null ? cursorRef.current : primaryId;
    const current = rows.findIndex((row) => row.id === cursorId);
    const meta = e.metaKey || e.ctrlKey;
    switch (e.key) {
      case 'ArrowDown':
        e.preventDefault();
        moveSelection(current < 0 ? 0 : current + 1, e.shiftKey);
        return;
      case 'ArrowUp':
        e.preventDefault();
        moveSelection(current < 0 ? 0 : current - 1, e.shiftKey);
        return;
      case 'Home':
        e.preventDefault();
        moveSelection(0, e.shiftKey);
        return;
      case 'End':
        e.preventDefault();
        moveSelection(rows.length - 1, e.shiftKey);
        return;
      case 'a':
        if (meta) {
          e.preventDefault();
          selectAll();
        }
        return;
      case ' ':
        if (selectedRows.length) {
          e.preventDefault();
          void actions.toggle(selectedRows);
        }
        return;
      case 'Enter':
        if (selectedRows.length === 1 && getDownloadCapabilities(selectedRows[0].status).open) {
          e.preventDefault();
          void actions.open(selectedRows[0]);
        }
        return;
      case 'Backspace':
      case 'Delete':
        if (selectedRows.length && (e.key === 'Delete' || meta)) {
          e.preventDefault();
          actions.remove(selectedRows);
        }
        return;
      case 'Escape':
        if (selectedRows.length) {
          e.preventDefault();
          select(null);
        }
        return;
      case 'ContextMenu':
      case 'F10':
        if ((e.key === 'ContextMenu' || e.shiftKey) && selectedRows.length) {
          e.preventDefault();
          openMenuForSelection();
        }
        return;
    }
  };

  const viewTitle = t(`views.${view}`);

  let body: React.ReactNode;
  if (loading && rows.length === 0) {
    body = (
      <div role="status" aria-label={t('list.loading')}>
        {[0, 1, 2, 3, 4, 5].map((i) => (
          <div className="skeleton-row" key={i}>
            <div />
            <div className="skeleton-lines">
              <div className="skeleton-line" style={{ width: `${62 - i * 5}%` }} />
              <div className="skeleton-line" style={{ width: `${38 + i * 4}%` }} />
            </div>
          </div>
        ))}
      </div>
    );
  } else if (error && rows.length === 0) {
    body = (
      <div className="empty" role="alert">
        <AlertTriangle size={40} strokeWidth={1.4} aria-hidden={true} />
        <h2>{t('list.load_error')}</h2>
        <p title={error}>{t('list.load_error_desc')}</p>
        <button type="button" className="btn" onClick={() => void refresh()}>
          {t('common.retry')}
        </button>
      </div>
    );
  } else if (rows.length === 0) {
    const Icon = search ? SearchX : (EMPTY_ICON[view] ?? Inbox);
    body = (
      <div className="empty">
        <Icon size={40} strokeWidth={1.4} aria-hidden={true} />
        <h2>{search ? t('list.no_results', { query: search }) : t(`list.empty_${view}`)}</h2>
        <p>{search ? t('list.no_results_desc') : t(`list.empty_${view}_desc`)}</p>
        {!search && (view === 'all' || view === 'active') && (
          <button type="button" className="btn primary" onClick={() => openNewDownload()}>
            {t('toolbar.new_download')}
          </button>
        )}
      </div>
    );
  } else {
    body = (
      <div className="list-spacer" style={{ height: virtualizer.getTotalSize() }}>
        {items.map((item) => {
          const row = rows[item.index];
          if (!row) return null;
          const selected = selectedIds.has(row.id);
          return (
            <div
              key={row.id}
              id={`dl-row-${row.id}`}
              className="list-row"
              role="option"
              aria-selected={selected}
              style={{ height: ROW_HEIGHT, transform: `translateY(${item.start}px)` }}
              onMouseDown={(e) => {
                if (e.button !== 0) return;
                if ((e.target as HTMLElement).closest('button')) return;
                select(row.id, { toggle: e.metaKey || e.ctrlKey, range: e.shiftKey });
                cursorRef.current = row.id;
              }}
              onDoubleClick={(e) => {
                if ((e.target as HTMLElement).closest('button')) return;
                const caps = getDownloadCapabilities(row.status);
                if (caps.open) void actions.open(row);
                else if (caps.resume) void actions.resume([row]);
              }}
              onContextMenu={(e) => {
                e.preventDefault();
                openMenu(row, e.clientX, e.clientY);
              }}
            >
              <DownloadRow
                row={row}
                selected={selected}
                queuePosition={positions.get(row.id)}
                now={row.retry_at != null ? now : 0}
                onToggle={onToggle}
                onReveal={onReveal}
                onMenu={openMenu}
              />
            </div>
          );
        })}
      </div>
    );
  }

  return (
    <>
      {error && rows.length > 0 && (
        <div className="notice" role="alert">
          <span title={error}>{t('list.refresh_error')}</span>
          <button type="button" className="btn small" onClick={() => void refresh()}>
            {t('common.retry')}
          </button>
        </div>
      )}
      <div
        ref={scrollRef}
        className="list"
        role={rows.length ? 'listbox' : undefined}
        aria-label={viewTitle}
        aria-multiselectable={rows.length ? true : undefined}
        aria-activedescendant={primaryId !== null ? `dl-row-${primaryId}` : undefined}
        tabIndex={0}
        onKeyDown={onKeyDown}
        onMouseDown={(e) => {
          // Click on empty space clears the selection, like Finder.
          if (!(e.target as HTMLElement).closest('.list-row')) select(null);
        }}
      >
        {body}
        {loadingMore && (
          <div className="list-more" role="status">
            {t('list.loading_more')}
          </div>
        )}
      </div>
      {menu && (
        <ContextMenu
          x={menu.x}
          y={menu.y}
          label={t('actions.menu_label')}
          entries={buildDownloadMenu(menu.rows, actions, t)}
          onClose={() => setMenu(null)}
        />
      )}
    </>
  );
}
