import { useState, useEffect, useRef } from "react";
import { invoke } from "@tauri-apps/api/core";
import { downloadDir } from "@tauri-apps/api/path";
import { useTranslation } from "react-i18next";
import { X, ChevronRight, Loader2 } from "lucide-react";
import type { SettingsModel } from "../types";
import { useModalA11y } from "../hooks/useModalA11y";

type ToastFn = (kind: "success" | "error" | "info", msg: string) => void;

interface NewDownloadModalProps {
  onClose: () => void;
  onSuccess?: () => void;
  onAdded: () => void;
  initialUrl?: string;
  showToast: ToastFn;
}

export default function NewDownloadModal({ onClose, onSuccess, onAdded, initialUrl, showToast }: NewDownloadModalProps) {
  const { t } = useTranslation();
  const panelRef = useRef<HTMLFormElement>(null);
  useModalA11y(panelRef, onClose);
  const [url, setUrl] = useState(initialUrl || "");
  const [filename, setFilename] = useState("");
  const [savePath, setSavePath] = useState("");
  const [showAdvanced, setShowAdvanced] = useState(false);
  const [referrer, setReferrer] = useState("");
  const [userAgent, setUserAgent] = useState("");
  const [cookies, setCookies] = useState("");
  const [submitting, setSubmitting] = useState(false);
  const [batchMode, setBatchMode] = useState(false);

  useEffect(() => {
    invoke<SettingsModel>("get_settings")
      .then((s) => {
        if (s.default_download_path) setSavePath(s.default_download_path);
        else downloadDir().then(setSavePath).catch(() => setSavePath("~/Downloads"));
      })
      .catch(() => downloadDir().then(setSavePath).catch(() => setSavePath("~/Downloads")));
  }, []);

  useEffect(() => {
    if (!filename && url && !batchMode) {
      const first = url.trim().split(/\s+/).filter(Boolean)[0] || "";
      const guess = first.split("/").pop()?.split("?")[0] || "download.bin";
      if (guess.includes(".")) setFilename(guess);
    }
  }, [url, filename, batchMode]);

  const handleSubmit = async (e: React.FormEvent) => {
    e.preventDefault();
    if (!url.trim() || submitting) return;
    setSubmitting(true);
    try {
      const urls = batchMode
        ? url.split(/[\n\s]+/).map((u) => u.trim()).filter((u) => /^https?:\/\//i.test(u))
        : [url.trim()];

      if (urls.length === 0) {
        showToast("error", t("newDownloadModal.add_failed") + ": URL geçersiz");
        return;
      }

      for (const u of urls) {
        const name =
          (!batchMode && filename.trim()) ||
          u.split("/").pop()?.split("?")[0] ||
          "download.bin";
        await invoke("add_download", {
          url: u,
          filename: name,
          savePath: savePath || "~/Downloads",
          referrer: referrer || null,
          userAgent: userAgent || null,
          cookies: cookies || null,
        });
      }
      showToast("success", t("newDownloadModal.added_success"));
      onAdded();
      onSuccess?.();
      onClose();
    } catch (err) {
      const detail =
        typeof err === "string"
          ? err
          : err && typeof err === "object" && "message" in err
            ? String((err as { message: unknown }).message)
            : String(err);
      showToast("error", `${t("newDownloadModal.add_failed")}: ${detail}`);
      console.error(err);
    } finally {
      setSubmitting(false);
    }
  };

  return (
    <div className="modal-overlay" onClick={onClose} role="presentation">
      <form
        ref={panelRef}
        className="modal-panel"
        style={{ width: 480 }}
        onClick={(e) => e.stopPropagation()}
        onSubmit={handleSubmit}
        role="dialog"
        aria-modal="true"
        aria-labelledby="new-dl-title"
      >
        <div className="modal-head">
          <h2 id="new-dl-title" className="modal-title">{t("newDownloadModal.title")}</h2>
          <button type="button" onClick={onClose} className="icon-btn" aria-label={t("newDownloadModal.cancel")}><X size={18} /></button>
        </div>

        <div className="modal-body">
          <div className="check-row">
            <input id="batch-mode" type="checkbox" checked={batchMode} onChange={(e) => setBatchMode(e.target.checked)} />
            <label htmlFor="batch-mode">{t("newDownloadModal.batch_mode")}</label>
          </div>

          <div className="field">
            <label className="field-label" htmlFor="dl-url">{t("newDownloadModal.url")}</label>
            {batchMode ? (
              <textarea
                id="dl-url"
                className="field-input"
                rows={5}
                placeholder={t("newDownloadModal.batch_placeholder")}
                value={url}
                onChange={(e) => setUrl(e.target.value)}
                autoFocus
              />
            ) : (
              <input
                id="dl-url"
                className="field-input"
                type="text"
                placeholder={t("newDownloadModal.url_placeholder")}
                value={url}
                onChange={(e) => setUrl(e.target.value)}
                autoFocus
              />
            )}
          </div>

          {!batchMode && (
            <div className="field">
              <label className="field-label" htmlFor="dl-filename">{t("newDownloadModal.filename")}</label>
              <input
                id="dl-filename"
                className="field-input"
                type="text"
                placeholder={t("newDownloadModal.filename_placeholder")}
                value={filename}
                onChange={(e) => setFilename(e.target.value)}
              />
            </div>
          )}

          <div className="field">
            <label className="field-label" htmlFor="dl-path">{t("newDownloadModal.save_path")}</label>
            <input
              id="dl-path"
              className="field-input"
              type="text"
              value={savePath}
              onChange={(e) => setSavePath(e.target.value)}
            />
          </div>

          <button type="button" className={`adv-toggle ${showAdvanced ? "open" : ""}`} onClick={() => setShowAdvanced(!showAdvanced)}>
            <ChevronRight size={14} />
            {t("newDownloadModal.advanced_options")}
          </button>

          {showAdvanced && (
            <div className="adv-fields">
              <div className="field">
                <label className="field-label" htmlFor="dl-ref">{t("newDownloadModal.referrer")}</label>
                <input id="dl-ref" className="field-input" type="text" value={referrer} onChange={(e) => setReferrer(e.target.value)} />
              </div>
              <div className="field">
                <label className="field-label" htmlFor="dl-ua">{t("newDownloadModal.user_agent")}</label>
                <input id="dl-ua" className="field-input" type="text" value={userAgent} onChange={(e) => setUserAgent(e.target.value)} />
              </div>
              <div className="field">
                <label className="field-label" htmlFor="dl-cookies">{t("newDownloadModal.cookies")}</label>
                <input id="dl-cookies" className="field-input" type="text" value={cookies} onChange={(e) => setCookies(e.target.value)} />
              </div>
            </div>
          )}
        </div>

        <div className="modal-foot">
          <button type="button" className="btn-secondary" onClick={onClose}>{t("newDownloadModal.cancel")}</button>
          <button type="submit" className="btn-primary" disabled={submitting || !url.trim()}>
            {submitting ? <Loader2 size={15} className="spin" /> : null}
            {submitting ? t("newDownloadModal.adding") : t("newDownloadModal.start_download")}
          </button>
        </div>
      </form>
    </div>
  );
}
