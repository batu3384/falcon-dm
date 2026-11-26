import { useId, useState } from 'react';
import { useTranslation } from 'react-i18next';
import { open as openDialog } from '@tauri-apps/plugin-dialog';
import { Sheet } from './Sheet';
import * as api from '../api/commands';
import { useToastStore } from '../store/toast';
import type { DownloadModel } from '../types';

export function MoveRenameDialog({ row, onClose }: { row: DownloadModel; onClose: () => void }) {
  const { t } = useTranslation();
  const showToast = useToastStore((s) => s.showToast);
  const nameId = useId();
  const pathId = useId();
  const [name, setName] = useState(row.filename);
  const [folder, setFolder] = useState(row.save_path);
  const [error, setError] = useState('');
  const [busy, setBusy] = useState(false);

  const submit = async (e: React.FormEvent) => {
    e.preventDefault();
    if (!name.trim()) return;
    setBusy(true);
    setError('');
    try {
      await api.moveDownload(
        row.id,
        name.trim() !== row.filename ? name.trim() : undefined,
        folder.trim() !== row.save_path ? folder.trim() : undefined,
      );
      showToast('success', t('move.done'));
      onClose();
    } catch (err) {
      setError(api.localizeDownloadError(api.extractTauriError(err), t));
    } finally {
      setBusy(false);
    }
  };

  const browse = async () => {
    try {
      const picked = await openDialog({ directory: true, multiple: false, defaultPath: folder });
      if (typeof picked === 'string') setFolder(picked);
    } catch (err) {
      showToast('error', api.extractTauriError(err));
    }
  };

  return (
    <Sheet
      as="form"
      title={t('move.title')}
      onClose={onClose}
      onSubmit={(e) => void submit(e)}
      footer={
        <>
          <button type="button" className="btn" data-modal-cancel onClick={onClose}>
            {t('common.cancel')}
          </button>
          <button type="submit" className="btn primary" disabled={busy || !name.trim()}>
            {t('move.submit')}
          </button>
        </>
      }
    >
      <div className="sheet-body">
        <div className="field">
          <label htmlFor={nameId}>{t('move.name')}</label>
          <input
            id={nameId}
            className="input"
            value={name}
            onChange={(e) => setName(e.target.value)}
            data-modal-initial-focus
            spellCheck={false}
          />
        </div>
        <div className="field">
          <label htmlFor={pathId}>{t('move.folder')}</label>
          <div className="input-row">
            <input
              id={pathId}
              className="input"
              value={folder}
              onChange={(e) => setFolder(e.target.value)}
              spellCheck={false}
            />
            <button type="button" className="btn" onClick={() => void browse()}>
              {t('common.choose')}
            </button>
          </div>
        </div>
        {error && (
          <p className="field-error" role="alert">
            {error}
          </p>
        )}
      </div>
    </Sheet>
  );
}
