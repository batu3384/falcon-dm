import {
  Archive,
  ArchiveRestore,
  ArrowDown,
  ArrowUp,
  Copy,
  ExternalLink,
  FolderInput,
  Pause,
  Play,
  RefreshCw,
  RotateCw,
  Search,
  Trash2,
} from 'lucide-react';
import type { DownloadModel } from '../types';
import { getDownloadCapabilities } from '../lib/downloadCapabilities';
import type { DownloadActions } from '../lib/useDownloadActions';
import type { MenuEntry } from './ContextMenu';

type Translate = (key: string, options?: Record<string, unknown>) => string;

const ICON = { size: 14, 'aria-hidden': true } as const;

/** Context menu for one row or a multi-selection. */
export function buildDownloadMenu(
  rows: DownloadModel[],
  actions: DownloadActions,
  t: Translate,
): MenuEntry[] {
  const entries: MenuEntry[] = [];
  const caps = rows.map((row) => getDownloadCapabilities(row.status));
  const many = rows.length > 1;
  const suffix = many ? ` (${rows.length})` : '';
  if (caps.some((c) => c.pause)) {
    entries.push({
      id: 'pause',
      label: t('actions.pause') + suffix,
      icon: <Pause {...ICON} />,
      run: () => void actions.pause(rows),
    });
  }
  if (caps.some((c) => c.resume)) {
    const onlyFailed = rows.every((row) => row.status !== 'Paused');
    entries.push({
      id: 'resume',
      label: (onlyFailed ? t('actions.retry') : t('actions.resume')) + suffix,
      icon: onlyFailed ? <RotateCw {...ICON} /> : <Play {...ICON} />,
      run: () => void actions.resume(rows),
    });
  }
  if (!many) {
    const row = rows[0];
    const cap = caps[0];
    if (entries.length) entries.push({ id: 'sep-state', separator: true });
    if (cap.open) {
      entries.push({
        id: 'open',
        label: t('actions.open'),
        icon: <ExternalLink {...ICON} />,
        run: () => void actions.open(row),
      });
    }
    entries.push({
      id: 'reveal',
      label: t('actions.reveal'),
      icon: <Search {...ICON} />,
      run: () => void actions.reveal(row),
    });
    entries.push({
      id: 'copy',
      label: t('actions.copy_link'),
      icon: <Copy {...ICON} />,
      run: () => void actions.copyUrl(row),
    });
    if (cap.refresh) {
      entries.push({
        id: 'refresh',
        label: t('actions.refresh_link'),
        icon: <RefreshCw {...ICON} />,
        run: () => actions.refresh(row),
      });
    }
    if (cap.move) {
      entries.push({
        id: 'move',
        label: t('actions.move_rename'),
        icon: <FolderInput {...ICON} />,
        run: () => actions.moveRename(row),
      });
    }
    if (cap.priority) {
      entries.push({ id: 'sep-priority', separator: true });
      entries.push({
        id: 'up',
        label: t('actions.priority_up'),
        icon: <ArrowUp {...ICON} />,
        run: () => void actions.priority(row, true),
      });
      entries.push({
        id: 'down',
        label: t('actions.priority_down'),
        icon: <ArrowDown {...ICON} />,
        run: () => void actions.priority(row, false),
      });
    }
    if (row.status === 'Completed') {
      entries.push({
        id: 'redownload',
        label: t('actions.redownload'),
        icon: <RotateCw {...ICON} />,
        run: () => void actions.redownload(row),
      });
    }
    if (cap.archive) {
      entries.push({
        id: 'archive',
        label: row.archived ? t('actions.unarchive') : t('actions.archive'),
        icon: row.archived ? <ArchiveRestore {...ICON} /> : <Archive {...ICON} />,
        run: () => void actions.archive(row, !row.archived),
      });
    }
  }
  if (caps.some((c) => c.remove)) {
    entries.push({ id: 'sep-remove', separator: true });
    entries.push({
      id: 'remove',
      label: t('actions.remove') + suffix,
      icon: <Trash2 {...ICON} />,
      danger: true,
      run: () => actions.remove(rows),
    });
  }
  return entries;
}
