import { invoke } from "@tauri-apps/api/core";
import { useState, useEffect, useRef } from "react";
import { open } from "@tauri-apps/plugin-dialog";
import { useTranslation } from "react-i18next";
import { X } from "lucide-react";
import type { SettingsModel } from "../types";
import { applyTheme } from "../types";
import { useModalA11y } from "../hooks/useModalA11y";

interface SettingsModalProps {
  onClose: () => void;
  showToast?: (kind: "success" | "error" | "info", msg: string) => void;
}

export const SettingsModal = ({ onClose, showToast }: SettingsModalProps) => {
  const { t } = useTranslation();
  const panelRef = useRef<HTMLDivElement>(null);
  useModalA11y(panelRef, onClose);
  const [activeTab, setActiveTab] = useState<"general" | "network">("general");
  const [settings, setSettings] = useState<SettingsModel>({
    theme: "system",
    default_download_path: "~/Downloads",
    max_concurrent_downloads: 3,
    max_connections_per_server: 16,
    proxy: null,
    api_token: "",
    speed_limit_kbps: 0,
    category_paths: {},
    allowed_extension_ids: [],
    ytdlp_path: "",
  });
  const [saveError, setSaveError] = useState("");
  const [pendingPair, setPendingPair] = useState<string | null>(null);

  useEffect(() => {
    invoke<SettingsModel>("get_settings")
      .then(setSettings)
      .catch((e) => console.error("Failed to load settings:", e));
    invoke<string | null>("get_pending_pair")
      .then(setPendingPair)
      .catch(() => {});
  }, []);

  useEffect(() => {
    let unlisten: (() => void) | undefined;
    import("@tauri-apps/api/event").then(({ listen }) => {
      listen<{ extension_id: string }>("pair-request", (e) => {
        setPendingPair(e.payload.extension_id);
      }).then((fn) => {
        unlisten = fn;
      });
    });
    return () => {
      unlisten?.();
    };
  }, []);

  const handleChange = (field: keyof SettingsModel, value: string | number | null) => {
    setSettings((prev) => ({ ...prev, [field]: value }));
  };

  const handleBrowse = async () => {
    try {
      const selected = await open({ directory: true, multiple: false });
      if (selected && typeof selected === "string") handleChange("default_download_path", selected);
    } catch (e) {
      console.error("Browse error:", e);
    }
  };

  const handleRelinkExtension = async () => {
    try {
      await invoke("reset_extension_pin");
      setPendingPair(null);
      showToast?.("success", t("settings.extension_reset"));
    } catch (e) {
      console.error(e);
      showToast?.("error", t("settings.save_failed"));
    }
  };

  const handleApprovePair = async () => {
    if (!pendingPair) return;
    try {
      await invoke("approve_extension_pair", { extensionId: pendingPair });
      setPendingPair(null);
      const s = await invoke<SettingsModel>("get_settings");
      setSettings(s);
      showToast?.("success", t("settings.pair_approved"));
    } catch (e) {
      console.error(e);
      showToast?.("error", t("settings.save_failed"));
    }
  };

  const handleSave = async () => {
    setSaveError("");
    try {
      await invoke("save_settings", { settings });
      applyTheme(settings.theme);
      showToast?.("success", t("settings.saved"));
      onClose();
    } catch (e) {
      console.error("Failed to save settings:", e);
      setSaveError(t("settings.save_failed"));
      showToast?.("error", t("settings.save_failed"));
    }
  };

  return (
    <div className="modal-overlay" onClick={onClose} role="presentation">
      <div ref={panelRef} className="modal-panel" style={{ width: 500 }} onClick={(e) => e.stopPropagation()} role="dialog" aria-modal="true" aria-labelledby="settings-title">
        <div className="modal-head">
          <h2 id="settings-title" className="modal-title">{t("settings.title")}</h2>
          <button type="button" onClick={onClose} className="icon-btn" aria-label={t("settings.cancel")}><X size={18} /></button>
        </div>

        <div className="modal-body">
          <div className="tabs" role="tablist">
            <button type="button" role="tab" aria-selected={activeTab === "general"} className={`tab ${activeTab === "general" ? "active" : ""}`} onClick={() => setActiveTab("general")}>
              {t("settings.general")}
            </button>
            <button type="button" role="tab" aria-selected={activeTab === "network"} className={`tab ${activeTab === "network" ? "active" : ""}`} onClick={() => setActiveTab("network")}>
              {t("settings.network")}
            </button>
          </div>

          {activeTab === "general" && (
            <>
              <div className="field">
                <label className="field-label" htmlFor="set-theme">{t("settings.theme")}</label>
                <select id="set-theme" className="field-input field-select" value={settings.theme} onChange={(e) => handleChange("theme", e.target.value)}>
                  <option value="system">{t("settings.theme_system")}</option>
                  <option value="light">{t("settings.theme_light")}</option>
                  <option value="dark">{t("settings.theme_dark")}</option>
                </select>
              </div>
              <div className="field">
                <label className="field-label" htmlFor="set-path">{t("settings.download_path")}</label>
                <div className="input-action">
                  <input id="set-path" className="field-input" type="text" value={settings.default_download_path} onChange={(e) => handleChange("default_download_path", e.target.value)} />
                  <button type="button" className="btn-secondary" onClick={handleBrowse}>{t("settings.browse")}</button>
                </div>
              </div>
              <div className="field">
                <label className="field-label" htmlFor="set-ytdlp">{t("settings.ytdlp_path")}</label>
                <input
                  id="set-ytdlp"
                  className="field-input"
                  type="text"
                  value={settings.ytdlp_path || ""}
                  onChange={(e) => handleChange("ytdlp_path", e.target.value)}
                  placeholder="/opt/homebrew/bin/yt-dlp"
                />
                <p className="field-hint">{t("settings.ytdlp_path_hint")}</p>
              </div>
              <div className="field">
                <label className="field-label" htmlFor="set-token">{t("settings.api_token")}</label>
                <input id="set-token" className="field-input" type="text" value={settings.api_token || ""} onChange={(e) => handleChange("api_token", e.target.value)} />
              </div>
              <div className="field">
                <label className="field-label">{t("settings.extension")}</label>
                <p className="field-hint">{t("settings.extension_hint")}</p>
                {pendingPair ? (
                  <div className="input-action" style={{ marginBottom: 8 }}>
                    <code className="field-hint" style={{ flex: 1 }}>{pendingPair}</code>
                    <button type="button" className="btn-primary" onClick={handleApprovePair}>
                      {t("settings.pair_approve")}
                    </button>
                  </div>
                ) : (
                  <p className="field-hint">{t("settings.pair_none")}</p>
                )}
                <button type="button" className="btn-secondary" onClick={handleRelinkExtension}>
                  {t("settings.relink_extension")}
                </button>
              </div>
              <div className="field">
                <label className="field-label" htmlFor="cat-video">{t("settings.cat_video")}</label>
                <input
                  id="cat-video"
                  className="field-input"
                  type="text"
                  value={settings.category_paths?.Video || ""}
                  onChange={(e) =>
                    setSettings((prev) => ({
                      ...prev,
                      category_paths: { ...(prev.category_paths || {}), Video: e.target.value },
                    }))
                  }
                  placeholder="~/Movies"
                />
              </div>
              <div className="field">
                <label className="field-label" htmlFor="cat-music">{t("settings.cat_music")}</label>
                <input
                  id="cat-music"
                  className="field-input"
                  type="text"
                  value={settings.category_paths?.Music || ""}
                  onChange={(e) =>
                    setSettings((prev) => ({
                      ...prev,
                      category_paths: { ...(prev.category_paths || {}), Music: e.target.value },
                    }))
                  }
                  placeholder="~/Music"
                />
              </div>
            </>
          )}

          {activeTab === "network" && (
            <>
              <div className="field">
                <label className="field-label" htmlFor="set-concurrent">{t("settings.max_concurrent")} ({settings.max_concurrent_downloads})</label>
                <input id="set-concurrent" type="range" min="1" max="10" value={settings.max_concurrent_downloads} onChange={(e) => handleChange("max_concurrent_downloads", parseInt(e.target.value))} className="field-range" />
              </div>
              <div className="field">
                <label className="field-label" htmlFor="set-conn">{t("settings.max_connections")}</label>
                <input id="set-conn" type="number" min="1" max="16" value={settings.max_connections_per_server} onChange={(e) => handleChange("max_connections_per_server", parseInt(e.target.value))} className="field-input" />
              </div>
              <div className="field">
                <label className="field-label" htmlFor="set-speed">{t("settings.speed_limit")}</label>
                <input id="set-speed" type="number" min="0" value={settings.speed_limit_kbps || 0} onChange={(e) => handleChange("speed_limit_kbps", parseInt(e.target.value) || 0)} className="field-input" />
              </div>
              <div className="field">
                <label className="field-label" htmlFor="set-proxy">{t("settings.proxy")}</label>
                <input id="set-proxy" type="text" placeholder={t("settings.proxy_placeholder")} value={settings.proxy || ""} onChange={(e) => handleChange("proxy", e.target.value || null)} className="field-input" />
              </div>
            </>
          )}
          {saveError && <p className="field-error">{saveError}</p>}
        </div>

        <div className="modal-foot">
          <button type="button" className="btn-secondary" onClick={onClose}>{t("settings.cancel")}</button>
          <button type="button" className="btn-primary" onClick={handleSave}>{t("settings.save")}</button>
        </div>
      </div>
    </div>
  );
};
