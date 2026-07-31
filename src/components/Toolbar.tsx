import { useState, useRef, useEffect } from "react";
import { useTranslation } from "react-i18next";
import { Plus, Play, Pause, Clock, Globe, Settings, Search, Clipboard } from "lucide-react";
import { SchedulerModal } from "./SchedulerModal";
import { SettingsModal } from "./SettingsModal";

interface ToolbarProps {
  onAddClick: () => void;
  searchQuery: string;
  onSearchChange: (q: string) => void;
  onPauseAll: () => void;
  onResumeAll: () => void;
  clipboardMonitor?: boolean;
  onToggleClipboard?: () => void;
}

export default function Toolbar({
  onAddClick,
  searchQuery,
  onSearchChange,
  onPauseAll,
  onResumeAll,
  clipboardMonitor,
  onToggleClipboard,
}: ToolbarProps) {
  const { t, i18n } = useTranslation();
  const [schedulerOpen, setSchedulerOpen] = useState(false);
  const [settingsOpen, setSettingsOpen] = useState(false);
  const searchRef = useRef<HTMLInputElement>(null);
  const isMac = navigator.platform.toUpperCase().includes("MAC");

  useEffect(() => {
    const handler = (e: KeyboardEvent) => {
      if ((e.metaKey || e.ctrlKey) && e.key === "k") {
        e.preventDefault();
        searchRef.current?.focus();
        searchRef.current?.select();
      }
    };
    window.addEventListener("keydown", handler);
    return () => window.removeEventListener("keydown", handler);
  }, []);

  const toggleLanguage = () => {
    const next = i18n.language === "en" ? "tr" : "en";
    i18n.changeLanguage(next);
    localStorage.setItem("falcon_lang", next);
    document.documentElement.lang = next;
  };

  return (
    <>
      <header className="toolbar no-drag">
        <div className="toolbar-row">
          <div className="toolbar-actions">
            <button type="button" className="btn-primary" onClick={onAddClick} aria-label={t("toolbar.add_download")}>
              <Plus size={15} strokeWidth={2.5} />
              <span>{t("toolbar.add_download")}</span>
            </button>

            <div className="toolbar-divider" />

            <button type="button" className="icon-btn" onClick={onResumeAll} title={t("toolbar.resume_all")} aria-label={t("toolbar.resume_all")}>
              <Play size={15} />
            </button>
            <button type="button" className="icon-btn" onClick={onPauseAll} title={t("toolbar.pause_all")} aria-label={t("toolbar.pause_all")}>
              <Pause size={15} />
            </button>
            <button type="button" className="icon-btn" onClick={() => setSchedulerOpen(true)} title={t("toolbar.scheduler")} aria-label={t("toolbar.scheduler")}>
              <Clock size={15} />
            </button>
            {onToggleClipboard && (
              <button
                type="button"
                className={`icon-btn ${clipboardMonitor ? "active" : ""}`}
                onClick={onToggleClipboard}
                title={t("toolbar.clipboard")}
                aria-label={t("toolbar.clipboard")}
                aria-pressed={clipboardMonitor}
              >
                <Clipboard size={15} />
              </button>
            )}
          </div>

          <div className="toolbar-actions">
            <div className="search-wrap">
              <Search size={14} />
              <input
                className="search-input"
                placeholder={t("toolbar.search_placeholder")}
                value={searchQuery}
                onChange={(e) => onSearchChange(e.target.value)}
                ref={searchRef}
                aria-label={t("toolbar.search_placeholder")}
              />
              <kbd className="kbd">{isMac ? "⌘K" : "Ctrl+K"}</kbd>
            </div>
            <button type="button" className="icon-btn lang-btn" onClick={toggleLanguage} title={t("sidebar.language")} aria-label={t("sidebar.language")}>
              <Globe size={15} />
              <span className="lang-tag">{i18n.language.toUpperCase()}</span>
            </button>

            <div className="toolbar-divider" />

            <button type="button" className="icon-btn" onClick={() => setSettingsOpen(true)} title={t("sidebar.settings")} aria-label={t("sidebar.settings")}>
              <Settings size={15} />
            </button>
          </div>
        </div>
      </header>

      <SchedulerModal isOpen={schedulerOpen} onClose={() => setSchedulerOpen(false)} />
      {settingsOpen && <SettingsModal onClose={() => setSettingsOpen(false)} />}
    </>
  );
}
