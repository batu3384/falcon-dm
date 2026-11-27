import { useId, useState } from 'react';
import { useTranslation } from 'react-i18next';
import { Loader2 } from 'lucide-react';
import { Sheet } from './Sheet';
import * as api from '../api/commands';
import { useToastStore } from '../store/toast';
import type { DownloadModel } from '../types';

/**
 * Paste a fresh link for a stopped download (signed CDN URLs expire).
 * The backend keeps the partial file, so the download resumes byte-exactly;
 * a different entity on the new link makes the engine start over.
 */
export function RefreshLinkDialog({ row, onClose }: { row: DownloadModel; onClose: () => void }) {
  const { t } = useTranslation();
  const showToast = useToastStore((s) => s.showToast);
  const urlId = useId();
  const errId = useId();
  const [url, setUrl] = useState('');
  const [error, setError] = useState('');
  const [busy, setBusy] = useState(false);

  const submit = async (e: React.FormEvent) => {
    e.preventDefault();
    const newUrl = url.trim();
    if (!newUrl || busy) return;
    setBusy(true);
    setError('');
    try {
      await api.refreshDownloadUrl(row.id, newUrl);
      showToast('success', t('refresh.done'));
      onClose();
    } catch (err) {
      setError(api.localizeDownloadError(api.extractTauriError(err), t));
    } finally {
      setBusy(false);
    }
  };

  return (
    <Sheet
      as="form"
      title={t('refresh.title')}
      description={t('refresh.hint', { name: row.filename })}
      onClose={onClose}
      onSubmit={(e) => void submit(e)}
      footer={
        <>
          <button type="button" className="btn" data-modal-cancel onClick={onClose}>
            {t('common.cancel')}
          </button>
          <button type="submit" className="btn primary" disabled={busy || !url.trim()}>
            {busy && <Loader2 size={14} className="spin" aria-hidden={true} />}
            {t('refresh.submit')}
          </button>
        </>
      }
    >
      <div className="sheet-body">
        <div className="field">
          <label htmlFor={urlId}>{t('refresh.new_url')}</label>
          <input
            id={urlId}
            className="input"
            value={url}
            placeholder="https://"
            onChange={(e) => {
              setUrl(e.target.value);
              setError('');
            }}
            data-modal-initial-focus
            spellCheck={false}
            autoComplete="off"
            aria-invalid={error ? true : undefined}
            aria-describedby={error ? errId : undefined}
          />
          {error && (
            <p id={errId} className="field-error" role="alert">
              {error}
            </p>
          )}
        </div>
      </div>
    </Sheet>
  );
}
