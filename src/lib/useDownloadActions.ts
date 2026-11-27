import { useMemo } from 'react';
import { useTranslation } from 'react-i18next';
import * as api from '../api/commands';
import type { DownloadModel } from '../types';
import { fileFullPath } from './format';
import { getDownloadCapabilities } from './downloadCapabilities';
import { useToastStore } from '../store/toast';
import { useUiStore } from '../store/ui';
import { suppressClipboardUrl } from '../hooks/useClipboardMonitor';

/** Every per-download action, shared by rows, inspector, menus and shortcuts. */
export function useDownloadActions() {
  const { t } = useTranslation();
  const showToast = useToastStore((s) => s.showToast);
  const askRemove = useUiStore((s) => s.askRemove);
  const setMoveTarget = useUiStore((s) => s.setMoveTarget);
  const setRefreshTarget = useUiStore((s) => s.setRefreshTarget);

  return useMemo(() => {
    const fail = (e: unknown) =>
      showToast('error', api.localizeDownloadError(api.extractTauriError(e), t));

    /** Runs `fn` for each row it applies to; one toast summarizes failures. */
    const batch = async (
      rows: DownloadModel[],
      applies: (row: DownloadModel) => boolean,
      fn: (row: DownloadModel) => Promise<unknown>,
    ) => {
      const targets = rows.filter(applies);
      const results = await Promise.allSettled(targets.map(fn));
      const failed = results.filter((r): r is PromiseRejectedResult => r.status === 'rejected');
      if (failed.length === 1) fail(failed[0].reason);
      else if (failed.length > 1)
        showToast('error', t('actions.batch_failed', { count: failed.length }));
    };

    return {
      pause: (rows: DownloadModel[]) =>
        batch(
          rows,
          (r) => getDownloadCapabilities(r.status).pause,
          (r) => api.pauseDownload(r.id),
        ),
      resume: (rows: DownloadModel[]) =>
        batch(
          rows,
          (r) => getDownloadCapabilities(r.status).resume,
          (r) => api.resumeDownload(r.id),
        ),
      /** Pause running rows, resume stopped ones. */
      toggle: (rows: DownloadModel[]) =>
        batch(
          rows,
          (r) =>
            getDownloadCapabilities(r.status).pause || getDownloadCapabilities(r.status).resume,
          (r) =>
            getDownloadCapabilities(r.status).pause
              ? api.pauseDownload(r.id)
              : api.resumeDownload(r.id),
        ),
      remove: (rows: DownloadModel[]) => askRemove(rows),
      reveal: (row: DownloadModel) => api.openFolder(fileFullPath(row)).catch(fail),
      open: (row: DownloadModel) => api.openFile(fileFullPath(row)).catch(fail),
      copyUrl: async (row: DownloadModel) => {
        const url = row.url.replace(/#falconfmt=.*$/, '');
        try {
          // Mark before the async write so an in-flight poll cannot mistake it for a user copy.
          suppressClipboardUrl(url);
          await navigator.clipboard.writeText(url);
          showToast('success', t('actions.link_copied'));
        } catch (e) {
          fail(e);
        }
      },
      archive: (row: DownloadModel, archived: boolean) =>
        api.archiveDownload(row.id, archived).catch(fail),
      priority: (row: DownloadModel, up: boolean) => api.changePriority(row.id, up).catch(fail),
      speedLimit: (row: DownloadModel, kbps: number) =>
        api.setDownloadSpeedLimit(row.id, kbps).catch(fail),
      moveRename: (row: DownloadModel) => setMoveTarget(row),
      refresh: (row: DownloadModel) => setRefreshTarget(row),
      redownload: async (row: DownloadModel) => {
        try {
          const added = await api.addDownload({
            url: row.url,
            filename: row.filename,
            savePath: row.save_path,
            referrer: row.referrer ?? undefined,
            userAgent: row.user_agent ?? undefined,
          });
          if (added.id === row.id) showToast('info', t('actions.already_queued'));
        } catch (e) {
          fail(e);
        }
      },
    };
  }, [askRemove, setMoveTarget, setRefreshTarget, showToast, t]);
}

export type DownloadActions = ReturnType<typeof useDownloadActions>;
