import { useState, useEffect, useRef, useCallback } from 'react';
import { downloadDir } from '@tauri-apps/api/path';
import { useTranslation } from 'react-i18next';
import { X, ChevronRight, Loader2, UploadCloud } from 'lucide-react';
import { useModalA11y } from '../hooks/useModalA11y';
import { useToastStore } from '../store/toast';
import * as api from '../api/commands';

// ponytail: parse a dropped .txt/.csv file into a list of http(s) URLs. Handles
// comma/newline/whitespace separated lists and tolerates leading/trailing junk.
function parseUrlsFromText(text: string): string[] {
  return text
    .split(/[\n\r,;\s]+/)
    .map((s) => s.trim())
    .filter((s) => /^https?:\/\//i.test(s));
}

function isHttpUrl(value: string): boolean {
  try {
    const parsed = new URL(value);
    return (parsed.protocol === 'http:' || parsed.protocol === 'https:') && !!parsed.hostname;
  } catch {
    return false;
  }
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
  const [batchMode, setBatchMode] = useState(false);
  const [dragActive, setDragActive] = useState(false);
  const requestClose = useCallback(() => {
    if (!submitting) onClose();
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
      const guess = first.split('/').pop()?.split('?')[0] || 'download.bin';
      if (guess.includes('.')) setFilename(guess);
    }
  }, [url, filename, batchMode]);

  const handleSubmit = async (e: React.FormEvent) => {
    e.preventDefault();
    if (!url.trim() || submitting) return;
    setSubmitting(true);
    try {
      const urls = batchMode
        ? url
            .split(/[\n\s]+/)
            .map((u) => u.trim())
            .filter(Boolean)
        : [url.trim()];

      if (urls.length === 0 || urls.some((candidate) => !isHttpUrl(candidate))) {
        showToast(
          'error',
          `${t('newDownloadModal.add_failed')}: ${t('newDownloadModal.url_invalid')}`,
        );
        return;
      }

      for (const u of urls) {
        const name =
          (!batchMode && filename.trim()) || u.split('/').pop()?.split('?')[0] || 'download.bin';
        await api.addDownload({
          url: u,
          filename: name,
          savePath: savePath || '~/Downloads',
          referrer: referrer || undefined,
          userAgent: userAgent || undefined,
          cookies: cookies || undefined,
        });
      }
      showToast('success', t('newDownloadModal.added_success'));
      onAdded();
      onSuccess?.();
      onClose();
    } catch (err) {
      // ponytail: centralized error extraction replaces the hand-rolled typeof/object check.
      const detail = api.extractTauriError(err);
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
            <X size={18} />
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
                role="button"
                tabIndex={0}
                aria-label={t('newDownloadModal.import_drop_hint')}
                onKeyDown={(e) => {
                  if (e.key === 'Enter') document.getElementById('import-file-input')?.click();
                }}
              >
                <UploadCloud size={20} />
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
                onChange={(e) => setUrl(e.target.value)}
                autoFocus
              />
            ) : (
              <input
                id="dl-url"
                className="field-input"
                type="text"
                placeholder={t('newDownloadModal.url_placeholder')}
                value={url}
                onChange={(e) => setUrl(e.target.value)}
                autoFocus
              />
            )}
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
                onChange={(e) => setFilename(e.target.value)}
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
              onChange={(e) => setSavePath(e.target.value)}
            />
          </div>

          <button
            type="button"
            className={`adv-toggle ${showAdvanced ? 'open' : ''}`}
            onClick={() => setShowAdvanced(!showAdvanced)}
          >
            <ChevronRight size={14} />
            {t('newDownloadModal.advanced_options')}
          </button>

          {showAdvanced && (
            <div className="adv-fields">
              <div className="field">
                <label className="field-label" htmlFor="dl-ref">
                  {t('newDownloadModal.referrer')}
                </label>
                <input
                  id="dl-ref"
                  className="field-input"
                  type="text"
                  value={referrer}
                  onChange={(e) => setReferrer(e.target.value)}
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
                  onChange={(e) => setUserAgent(e.target.value)}
                />
              </div>
              <div className="field">
                <label className="field-label" htmlFor="dl-cookies">
                  {t('newDownloadModal.cookies')}
                </label>
                <input
                  id="dl-cookies"
                  className="field-input"
                  type="text"
                  value={cookies}
                  onChange={(e) => setCookies(e.target.value)}
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
            {submitting ? <Loader2 size={15} className="spin" /> : null}
            {submitting ? t('newDownloadModal.adding') : t('newDownloadModal.start_download')}
          </button>
        </div>
      </form>
    </div>
  );
}
