import { useCallback, useEffect, useId, useMemo, useRef, useState } from 'react';
import { useTranslation } from 'react-i18next';
import { open as openDialog } from '@tauri-apps/plugin-dialog';
import { ChevronRight, Loader2 } from 'lucide-react';
import { Sheet } from './Sheet';
import { ConfirmDialog } from './ConfirmDialog';
import { useToastStore } from '../store/toast';
import * as api from '../api/commands';
import {
  enqueueCookieUrl,
  isYoutubePageUrl,
  mixedTextLinkStats,
  publicHttpUrlsFromMixedText,
} from '../lib/publicHttpUrl';
import { canonicalizeChecksum } from '../lib/checksum';
import { YT_QUALITIES, ytFormatFor, type YtQualityId } from '../lib/ytFormats';
import { displayPath } from '../types';

interface NewDownloadSheetProps {
  initialText: string;
  onClose: () => void;
  onAdded?: () => void;
}

const QUALITY_KEY = 'falcon_yt_quality';

function savedQuality(): YtQualityId {
  const saved = localStorage.getItem(QUALITY_KEY);
  return (YT_QUALITIES.find((q) => q.id === saved)?.id ?? 'best') as YtQualityId;
}

export default function NewDownloadSheet({ initialText, onClose, onAdded }: NewDownloadSheetProps) {
  const { t } = useTranslation();
  const showToast = useToastStore((s) => s.showToast);
  const ids = { url: useId(), name: useId(), path: useId(), quality: useId(), err: useId() };
  const [text, setText] = useState(initialText);
  const [filename, setFilename] = useState('');
  const [savePath, setSavePath] = useState('');
  const [quality, setQuality] = useState<YtQualityId>(savedQuality);
  const [advanced, setAdvanced] = useState(false);
  const [referrer, setReferrer] = useState('');
  const [userAgent, setUserAgent] = useState('');
  const [cookies, setCookies] = useState('');
  const [checksum, setChecksum] = useState('');
  const [checksumError, setChecksumError] = useState('');
  const [submitting, setSubmitting] = useState(false);
  const [error, setError] = useState('');
  const [dragOver, setDragOver] = useState(false);
  const [confirmClose, setConfirmClose] = useState(false);
  const dirty = useRef(false);
  const fileInput = useRef<HTMLInputElement>(null);

  const { urls, skippedLines } = useMemo(() => mixedTextLinkStats(text), [text]);
  const single = urls.length <= 1;
  const hasYoutube = urls.some(isYoutubePageUrl);

  useEffect(() => {
    api
      .getSettings()
      .then((s) => setSavePath((current) => current || s.default_download_path))
      .catch(() => setSavePath((current) => current || '~/Downloads'));
  }, []);

  const requestClose = useCallback(() => {
    if (submitting) return;
    if (dirty.current) setConfirmClose(true);
    else onClose();
  }, [onClose, submitting]);

  const edit =
    <T,>(setter: (value: T) => void) =>
    (value: T) => {
      dirty.current = true;
      setter(value);
    };

  const importFile = async (file: File | undefined) => {
    if (!file) return;
    if (!/\.(txt|csv)$/i.test(file.name)) {
      showToast('error', t('new_download.import_unsupported'));
      return;
    }
    try {
      const found = publicHttpUrlsFromMixedText(await file.text());
      if (!found.length) {
        showToast('error', t('new_download.import_empty'));
        return;
      }
      dirty.current = true;
      setText((current) => [current.trim(), ...found].filter(Boolean).join('\n'));
      setError('');
    } catch {
      showToast('error', t('new_download.import_failed'));
    }
  };

  const browse = async () => {
    try {
      const picked = await openDialog({ directory: true, multiple: false, defaultPath: savePath });
      if (typeof picked === 'string') edit(setSavePath)(picked);
    } catch (e) {
      showToast('error', api.extractTauriError(e));
    }
  };

  const submit = async (e: React.FormEvent) => {
    e.preventDefault();
    if (submitting) return;
    if (!urls.length) {
      setError(t('new_download.invalid_url'));
      return;
    }
    // A checksum only makes sense for one file; lenient input is validated
    // here and re-validated (fail-closed) at enqueue.
    const expectedHash = single ? canonicalizeChecksum(checksum) : null;
    if (single && checksum.trim() && !expectedHash) {
      setChecksumError(t('errors.checksum_invalid'));
      return;
    }
    setSubmitting(true);
    setError('');
    localStorage.setItem(QUALITY_KEY, quality);
    const cookieUrl = enqueueCookieUrl(cookies, referrer, urls);
    const results = await Promise.allSettled(
      urls.map((url) =>
        api.addDownload({
          url,
          filename: single && filename.trim() ? filename.trim() : undefined,
          savePath: savePath.trim() || undefined,
          referrer: referrer.trim() || undefined,
          userAgent: userAgent.trim() || undefined,
          cookies: cookies.trim() || undefined,
          cookieUrl,
          format: isYoutubePageUrl(url) ? ytFormatFor(quality) : undefined,
          expectedHash: expectedHash ?? undefined,
        }),
      ),
    );
    setSubmitting(false);
    const failures = results.flatMap((result, index) =>
      result.status === 'rejected'
        ? [
            {
              url: urls[index],
              reason: api.localizeDownloadError(api.extractTauriError(result.reason), t),
            },
          ]
        : [],
    );
    const added = results.length - failures.length;
    if (added) onAdded?.();
    if (!failures.length) {
      showToast(
        'success',
        skippedLines
          ? t('new_download.added_skipped', { added, skipped: skippedLines })
          : t('new_download.added', { count: added }),
      );
      onClose();
      return;
    }
    if (single) {
      setError(failures[0].reason);
      return;
    }
    showToast(
      added ? 'info' : 'error',
      skippedLines
        ? t('new_download.partial_skipped', {
            added,
            failed: failures.length,
            skipped: skippedLines,
            reason: failures[0].reason,
          })
        : t('new_download.partial', {
            added,
            failed: failures.length,
            reason: failures[0].reason,
          }),
    );
    if (added) onClose();
  };

  const countParts: string[] = [];
  if (urls.length > 1) countParts.push(t('new_download.link_count', { count: urls.length }));
  if (urls.length && skippedLines) {
    countParts.push(t('new_download.skipped_count', { count: skippedLines }));
  }
  const countLabel =
    countParts.join(' · ') || (text.trim() && !urls.length ? t('new_download.no_link') : '');

  return (
    <>
      <Sheet
        as="form"
        title={t('new_download.title')}
        onClose={requestClose}
        onSubmit={(e) => void submit(e)}
        footer={
          <>
            <button
              type="button"
              className="btn"
              onClick={() => fileInput.current?.click()}
              title={t('new_download.import_hint')}
            >
              {t('new_download.import')}
            </button>
            <span className="grow" />
            <button type="button" className="btn" data-modal-cancel onClick={requestClose}>
              {t('common.cancel')}
            </button>
            <button type="submit" className="btn primary" disabled={submitting || !text.trim()}>
              {submitting && <Loader2 size={14} className="spin" aria-hidden={true} />}
              {urls.length > 1
                ? t('new_download.submit_many', { count: urls.length })
                : t('new_download.submit')}
            </button>
          </>
        }
      >
        <div className="sheet-body">
          <div
            className={`field dropzone${dragOver ? ' active' : ''}`}
            onDragOver={(e) => {
              if (!e.dataTransfer.types.includes('Files')) return;
              e.preventDefault();
              e.stopPropagation();
              setDragOver(true);
            }}
            onDragLeave={() => setDragOver(false)}
            onDrop={(e) => {
              if (!e.dataTransfer.files.length) return;
              e.preventDefault();
              e.stopPropagation();
              setDragOver(false);
              void importFile(e.dataTransfer.files[0]);
            }}
          >
            <div className="field-label-row">
              <label htmlFor={ids.url}>{t('new_download.url')}</label>
              {countLabel && <span className="hint">{countLabel}</span>}
            </div>
            <textarea
              id={ids.url}
              className="textarea"
              rows={single ? 2 : 5}
              placeholder={t('new_download.url_placeholder')}
              value={text}
              onChange={(e) => {
                edit(setText)(e.target.value);
                setError('');
              }}
              onKeyDown={(e) => {
                // ↩ submits a single link, ⌘↩ a list; ⇧↩ always adds a line.
                if (e.key !== 'Enter' || e.shiftKey) return;
                if (e.metaKey || e.ctrlKey || !text.includes('\n')) {
                  e.preventDefault();
                  e.currentTarget.form?.requestSubmit();
                }
              }}
              spellCheck={false}
              autoComplete="off"
              data-modal-initial-focus
              aria-invalid={error ? true : undefined}
              aria-describedby={error ? ids.err : undefined}
            />
            {error && (
              <p id={ids.err} className="field-error" role="alert">
                {error}
              </p>
            )}
            <input
              ref={fileInput}
              type="file"
              accept=".txt,.csv,text/plain,text/csv"
              hidden
              onChange={(e) => {
                void importFile(e.target.files?.[0]);
                e.target.value = '';
              }}
            />
          </div>

          {single && (
            <div className="field">
              <label htmlFor={ids.name}>{t('new_download.filename')}</label>
              <input
                id={ids.name}
                className="input"
                value={filename}
                placeholder={t('new_download.filename_placeholder')}
                onChange={(e) => edit(setFilename)(e.target.value)}
                spellCheck={false}
              />
            </div>
          )}

          <div className="field">
            <label htmlFor={ids.path}>{t('new_download.save_to')}</label>
            <div className="input-row">
              <input
                id={ids.path}
                className="input"
                value={savePath}
                title={savePath}
                onChange={(e) => edit(setSavePath)(e.target.value)}
                spellCheck={false}
              />
              <button type="button" className="btn" onClick={() => void browse()}>
                {t('common.choose')}
              </button>
            </div>
            {savePath && savePath !== displayPath(savePath) && (
              <p className="hint">{displayPath(savePath)}</p>
            )}
          </div>

          {hasYoutube && (
            <div className="field">
              <label htmlFor={ids.quality}>{t('new_download.quality')}</label>
              <select
                id={ids.quality}
                className="select"
                value={quality}
                onChange={(e) => setQuality(e.target.value as YtQualityId)}
              >
                {YT_QUALITIES.map((q) => (
                  <option key={q.id} value={q.id}>
                    {t(`quality.${q.id}`)}
                  </option>
                ))}
              </select>
            </div>
          )}

          <button
            type="button"
            className="disclosure"
            aria-expanded={advanced}
            onClick={() => setAdvanced((v) => !v)}
          >
            <ChevronRight size={13} aria-hidden={true} />
            {t('new_download.advanced')}
          </button>
          {advanced && (
            <>
              <div className="field">
                <label htmlFor={`${ids.url}-ref`}>{t('new_download.referrer')}</label>
                <input
                  id={`${ids.url}-ref`}
                  className="input"
                  value={referrer}
                  placeholder="https://"
                  onChange={(e) => edit(setReferrer)(e.target.value)}
                  spellCheck={false}
                />
              </div>
              <div className="field">
                <label htmlFor={`${ids.url}-ua`}>{t('new_download.user_agent')}</label>
                <input
                  id={`${ids.url}-ua`}
                  className="input"
                  value={userAgent}
                  onChange={(e) => edit(setUserAgent)(e.target.value)}
                  spellCheck={false}
                />
              </div>
              <div className="field">
                <label htmlFor={`${ids.url}-ck`}>{t('new_download.cookies')}</label>
                <input
                  id={`${ids.url}-ck`}
                  className="input"
                  type="password"
                  autoComplete="off"
                  value={cookies}
                  placeholder="name=value; other=value"
                  onChange={(e) => edit(setCookies)(e.target.value)}
                />
                <p className="hint">{t('new_download.cookies_hint')}</p>
              </div>
              {single && (
                <div className="field">
                  <label htmlFor={`${ids.url}-sum`}>{t('new_download.checksum')}</label>
                  <input
                    id={`${ids.url}-sum`}
                    className="input"
                    value={checksum}
                    placeholder="sha256:…"
                    onChange={(e) => {
                      edit(setChecksum)(e.target.value);
                      setChecksumError('');
                    }}
                    spellCheck={false}
                    autoComplete="off"
                    aria-invalid={checksumError ? true : undefined}
                  />
                  {checksumError ? (
                    <p className="field-error" role="alert">
                      {checksumError}
                    </p>
                  ) : (
                    <p className="hint">{t('new_download.checksum_hint')}</p>
                  )}
                </div>
              )}
            </>
          )}
        </div>
      </Sheet>
      {confirmClose && (
        <ConfirmDialog
          message={t('new_download.discard_title')}
          confirmLabel={t('common.discard')}
          cancelLabel={t('common.keep_editing')}
          onConfirm={() => {
            setConfirmClose(false);
            onClose();
          }}
          onCancel={() => setConfirmClose(false)}
        />
      )}
    </>
  );
}
