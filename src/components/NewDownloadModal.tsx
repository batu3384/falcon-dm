import { useState, useEffect, useRef, useCallback } from 'react';
import { downloadDir } from '@tauri-apps/api/path';
import { useTranslation } from 'react-i18next';
import { X, ChevronRight, Loader2, UploadCloud } from 'lucide-react';
import { useModalA11y } from '../hooks/useModalA11y';
import { ConfirmDialog } from './ConfirmDialog';
import { useToastStore } from '../store/toast';
import * as api from '../api/commands';
import { isPublicHttpUrl, publicHttpUrlsFromMixedText, enqueueCookieUrl } from '../lib/publicHttpUrl';

function parseUrlsFromText(text: string): string[] {
  return publicHttpUrlsFromMixedText(text);
}

interface NewDownloadModalProps {
  onClose: () => void;
  onSuccess?: () => void;
  onAdded: () => void;
  initialUrl?: string;
}

export default function NewDownloadModal({
  onClose,
  onSuccess,
  onAdded,
  initialUrl,
}: NewDownloadModalProps) {
  const { t } = useTranslation();
  const showToast = useToastStore((s) => s.showToast);
  const panelRef = useRef<HTMLFormElement>(null);
  const [url, setUrl] = useState(initialUrl || '');
  const [filename, setFilename] = useState('');
  const [savePath, setSavePath] = useState('');
  const [showAdvanced, setShowAdvanced] = useState(false);
  const [referrer, setReferrer] = useState('');
  const [userAgent, setUserAgent] = useState('');
  const [cookies, setCookies] = useState('');
  const [submitting, setSubmitting] = useState(false);
  const [batchMode, setBatchMode] = useState(
    () => (initialUrl || '').split(/[\n\r]+/).filter(Boolean).length > 1,
  );
  const [dragActive, setDragActive] = useState(false);
  const [urlError, setUrlError] = useState('');
  const dirtyRef = useRef(false);
  const [confirmClose, setConfirmClose] = useState(false);
  const markDirty = () => {
    dirtyRef.current = true;
  };
  const requestClose = useCallback(() => {
    if (submitting) return;
    if (dirtyRef.current) setConfirmClose(true);
    else onClose();
  }, [onClose, submitting]);
  useModalA11y(panelRef, requestClose);

  // ponytail: handle a dropped .txt/.csv — read it as text, parse out URLs, fill
  // the batch textarea and switch to batch mode automatically.
  const handleFileDrop = async (files: FileList | null) => {
    if (!files || files.length === 0) return;
    const file = files[0];
    const name = file.name.toLowerCase();
    if (!name.endsWith('.txt') && !name.endsWith('.csv')) {
      showToast('error', t('newDownloadModal.import_unsupported'));
      return;
    }
    try {
      const text = await file.text();
      const urls = parseUrlsFromText(text);
      if (urls.length === 0) {
        showToast('error', t('newDownloadModal.import_no_urls'));
        return;
      }
      setBatchMode(true);
      setUrl(urls.join('\n'));
      dirtyRef.current = true;
      showToast('success', t('newDownloadModal.imported_count', { count: urls.length }));
    } catch {
      showToast('error', t('newDownloadModal.import_failed'));
    }
  };

  useEffect(() => {
    api
      .getSettings()
      .then((s) => {
        if (s.default_download_path) setSavePath(s.default_download_path);
        else
          downloadDir()
            .then(setSavePath)
            .catch(() => setSavePath('~/Downloads'));
      })
      .catch(() =>
        downloadDir()
          .then(setSavePath)
          .catch(() => setSavePath('~/Downloads')),
      );
  }, []);

  useEffect(() => {
    if (!filename && url && !batchMode) {
      const first = url.trim().split(/\s+/).filter(Boolean)[0] || '';
      try {
        const path = new URL(first).pathname.split('/').pop() || '';
        const guess = decodeURIComponent(path).split('?')[0];
        if (guess.includes('.')) setFilename(guess.slice(0, 200));
      } catch {
        /* ignore malformed URL while typing */
      }
    }
  }, [url, filename, batchMode]);

  const handleSubmit = async (e: React.FormEvent) => {
    e.preventDefault();
    if (!url.trim() || submitting) return;
    setSubmitting(true);
    setUrlError('');
    try {
      const urls = batchMode
        ? parseUrlsFromText(url)
        : isPublicHttpUrl(url.trim())
          ? [url.trim()]
          : [];

      if (urls.length === 0) {
        const detail = t('newDownloadModal.url_invalid');
        setUrlError(detail);
        showToast('error', `${t('newDownloadModal.add_failed')}: ${detail}`);
        return;
      }

      const cookieUrl = enqueueCookieUrl(cookies, referrer, urls);
      const results = await Promise.allSettled(
        urls.map((u) => {
          const name =
            (!batchMode && filename.trim()) || u.split('/').pop()?.split('?')[0] || 'download.bin';
          return api.addDownload({
            url: u,
            filename: name,
            savePath: savePath || '~/Downloads',
            referrer: referrer || undefined,
            userAgent: userAgent || undefined,
            cookies: cookies || undefined,
            cookieUrl,
          });
        }),
      );
      const succeeded = results.filter((result) => result.status === 'fulfilled');
      const failures = results.flatMap((result, index) =>
        result.status === 'rejected'
          ? [
              `${urls[index]} (${api.localizeDownloadError(api.extractTauriError(result.reason), t)})`,
            ]
          : [],
      );
      if (succeeded.length) {
        onAdded();
        onSuccess?.();
      }
      if (failures.length === 0) {
        showToast('success', t('newDownloadModal.added_success'));
        onClose();
        return;
      }
      showToast(
        succeeded.length ? 'info' : 'error',
        t('app.batch_result', {
          success: succeeded.length,
          failed: failures.length,
          names: `: ${failures.join(', ')}`,
        }),
      );
      if (succeeded.length) onClose();
    } catch (err) {
      // ponytail: centralized error extraction replaces the hand-rolled typeof/object check.
      const detail = api.localizeDownloadError(api.extractTauriError(err), t);
      showToast('error', `${t('newDownloadModal.add_failed')}: ${detail}`);
      console.error(err);
    } finally {
      setSubmitting(false);
    }
  };

  return (
    <div className="modal-overlay" onClick={requestClose} role="presentation">
      <form
        ref={panelRef}
        className="modal-panel modal-md"
        onClick={(e) => e.stopPropagation()}
        onSubmit={handleSubmit}
        role="dialog"
        aria-modal="true"
        aria-labelledby="new-dl-title"
      >
        <div className="modal-head">
          <h2 id="new-dl-title" className="modal-title">
            {t('newDownloadModal.title')}
          </h2>
          <button
            type="button"
            onClick={requestClose}
            className="icon-btn"
            data-modal-cancel
            aria-label={t('newDownloadModal.cancel')}
          >
            <X size={18} aria-hidden={true} />
          </button>
        </div>

        <div className="modal-body">
          <div className="check-row">
            <input
              id="batch-mode"
              type="checkbox"
              checked={batchMode}
              onChange={(e) => setBatchMode(e.target.checked)}
            />
            <label htmlFor="batch-mode">{t('newDownloadModal.batch_mode')}</label>
          </div>

          <div className="field">
            <label className="field-label" htmlFor="dl-url">
              {t('newDownloadModal.url')}
            </label>
            {batchMode && (
              // ponytail: drop zone for .txt/.csv URL lists. Dragging a file in
              // parses it and fills the textarea — no manual paste needed.
              <div
                className={`import-dropzone ${dragActive ? 'active' : ''}`}
                onDragOver={(e) => {
                  e.preventDefault();
                  setDragActive(true);
                }}
                onDragLeave={() => setDragActive(false)}
                onDrop={(e) => {
                  e.preventDefault();
                  setDragActive(false);
                  handleFileDrop(e.dataTransfer.files);
                }}
              >
                <UploadCloud size={20} aria-hidden={true} />
                <span>{t('newDownloadModal.import_drop_hint')}</span>
                <label className="import-browse">
                  {t('newDownloadModal.import_browse')}
                  <input
                    id="import-file-input"
                    type="file"
                    accept=".txt,.csv,text/plain,text/csv"
                    className="import-file-input"
                    onChange={(e) => handleFileDrop(e.target.files)}
                  />
                </label>
              </div>
            )}
            {batchMode ? (
              <textarea
                id="dl-url"
                className="field-input"
                rows={5}
                placeholder={t('newDownloadModal.batch_placeholder')}
                value={url}
                onChange={(e) => {
                  setUrl(e.target.value);
                  markDirty();
                  if (urlError) setUrlError('');
                }}
                autoFocus
                data-modal-initial-focus
                aria-invalid={urlError ? true : undefined}
                aria-describedby={urlError ? 'dl-url-error' : undefined}
              />
            ) : (
              <input
                id="dl-url"
                className="field-input"
                type="text"
                inputMode="url"
                placeholder={t('newDownloadModal.url_placeholder')}
                value={url}
                onChange={(e) => {
                  setUrl(e.target.value);
                  markDirty();
                  if (urlError) setUrlError('');
                }}
                autoFocus
                data-modal-initial-focus
                aria-invalid={urlError ? true : undefined}
                aria-describedby={urlError ? 'dl-url-error' : undefined}
              />
            )}
            {urlError ? (
              <p id="dl-url-error" className="field-error" role="alert">
                {urlError}
              </p>
            ) : null}
          </div>

          {!batchMode && (
            <div className="field">
              <label className="field-label" htmlFor="dl-filename">
                {t('newDownloadModal.filename')}
              </label>
              <input
                id="dl-filename"
                className="field-input"
                type="text"
                placeholder={t('newDownloadModal.filename_placeholder')}
                value={filename}
                onChange={(e) => {
                  markDirty();
                  setFilename(e.target.value);
                }}
              />
            </div>
          )}

          <div className="field">
            <label className="field-label" htmlFor="dl-path">
              {t('newDownloadModal.save_path')}
            </label>
            <input
              id="dl-path"
              className="field-input"
              type="text"
              value={savePath}
              onChange={(e) => {
                markDirty();
                setSavePath(e.target.value);
              }}
            />
          </div>

          <button
            type="button"
            className={`adv-toggle ${showAdvanced ? 'open' : ''}`}
            onClick={() => setShowAdvanced(!showAdvanced)}
            aria-expanded={showAdvanced}
            aria-controls="dl-adv"
          >
            <ChevronRight size={14} aria-hidden={true} />
            {t('newDownloadModal.advanced_options')}
          </button>

          {showAdvanced && (
            <div className="adv-fields" id="dl-adv">
              <div className="field">
                <label className="field-label" htmlFor="dl-ref">
                  {t('newDownloadModal.referrer')}
                </label>
                <input
                  id="dl-ref"
                  className="field-input"
                  type="text"
                  value={referrer}
                  onChange={(e) => {
                    markDirty();
                    setReferrer(e.target.value);
                  }}
                />
              </div>
              <div className="field">
                <label className="field-label" htmlFor="dl-ua">
                  {t('newDownloadModal.user_agent')}
                </label>
                <input
                  id="dl-ua"
                  className="field-input"
                  type="text"
                  value={userAgent}
                  onChange={(e) => {
                    markDirty();
                    setUserAgent(e.target.value);
                  }}
                />
              </div>
              <div className="field">
                <label className="field-label" htmlFor="dl-cookies">
                  {t('newDownloadModal.cookies')}
                </label>
                <input
                  id="dl-cookies"
                  className="field-input"
                  type="password"
                  autoComplete="off"
                  value={cookies}
                  onChange={(e) => {
                    markDirty();
                    setCookies(e.target.value);
                  }}
                />
              </div>
            </div>
          )}
        </div>

        <div className="modal-foot">
          <button type="button" className="btn-secondary" data-modal-cancel onClick={requestClose}>
            {t('newDownloadModal.cancel')}
          </button>
          <button type="submit" className="btn-primary" disabled={submitting || !url.trim()}>
            {submitting ? <Loader2 size={15} className="spin" aria-hidden={true} /> : null}
            {submitting ? t('newDownloadModal.adding') : t('newDownloadModal.start_download')}
          </button>
        </div>
      </form>
      {confirmClose && (
        <ConfirmDialog
          message={t('newDownloadModal.confirm_close')}
          confirmLabel={t('settings.discard')}
          cancelLabel={t('settings.continue_editing')}
          onConfirm={() => {
            dirtyRef.current = false;
            setConfirmClose(false);
            onClose();
          }}
          onCancel={() => setConfirmClose(false)}
        />
      )}
    </div>
  );
}
