import { useState, useEffect, useCallback } from "react";
import { listen } from "@tauri-apps/api/event";
import { invoke } from "@tauri-apps/api/core";
import { CheckCircle2, AlertCircle, Info } from "lucide-react";
import { useTranslation } from "react-i18next";
import "./App.css";
import TitleBar from "./components/TitleBar";
import Sidebar from "./components/Sidebar";
import Toolbar from "./components/Toolbar";
import DownloadList from "./components/DownloadList";
import NewDownloadModal from "./components/NewDownloadModal";
import { OnboardingWizard } from "./components/OnboardingWizard";
import { InspectorPanel } from "./components/InspectorPanel";
import StatusBar from "./components/StatusBar";
import type { DownloadModel, ProgressPayload } from "./types";
import { applyTheme } from "./types";

type ToastKind = "success" | "error" | "info";
interface Toast { id: number; kind: ToastKind; msg: string; }

const URL_RE = /^https?:\/\/\S+/i;

function App() {
  const { t } = useTranslation();
  const [showOnboarding, setShowOnboarding] = useState(false);
  const [isModalOpen, setIsModalOpen] = useState(false);
  const [prefilledUrl, setPrefilledUrl] = useState("");
  const [activeCategory, setActiveCategory] = useState("All Downloads");
  const [selectedDownload, setSelectedDownload] = useState<DownloadModel | null>(null);
  const [downloads, setDownloads] = useState<DownloadModel[]>([]);
  const [searchQuery, setSearchQuery] = useState("");
  const [toasts, setToasts] = useState<Toast[]>([]);
  const [clipboardMonitor, setClipboardMonitor] = useState(false);
  const [lastClipboard, setLastClipboard] = useState("");

  const showToast = useCallback((kind: ToastKind, msg: string) => {
    const id = Date.now();
    setToasts((prev) => [...prev, { id, kind, msg }]);
    setTimeout(() => setToasts((prev) => prev.filter((tt) => tt.id !== id)), 3500);
  }, []);

  const fetchDownloads = useCallback(async () => {
    try {
      const data = await invoke<DownloadModel[]>("get_downloads", { filter: {} });
      setDownloads(data);
    } catch (e) {
      console.error("Failed to fetch downloads", e);
    }
  }, []);

  // Keep inspector in sync with live list
  useEffect(() => {
    if (!selectedDownload) return;
    const live = downloads.find((d) => d.id === selectedDownload.id);
    if (live && live !== selectedDownload) setSelectedDownload(live);
    if (!live) setSelectedDownload(null);
  }, [downloads, selectedDownload]);

  useEffect(() => {
    const onboardingComplete = localStorage.getItem("onboarding_complete");
    if (!onboardingComplete) setShowOnboarding(true);

    invoke<{ theme: string }>("get_settings")
      .then((s) => applyTheme(s.theme))
      .catch(() => applyTheme("system"));

    fetchDownloads();

    const refreshTimer = setInterval(fetchDownloads, 5000);

    const unlistenAdded = listen<DownloadModel>("download-added", (event) => {
      setDownloads((prev) => {
        if (prev.some((d) => d.id === event.payload.id)) return prev;
        return [event.payload, ...prev];
      });
      showToast("success", t("app.download_captured", { name: event.payload.filename }));
    });

    const unlistenProgress = listen<ProgressPayload>("download-progress", (event) => {
      const p = event.payload;
      setDownloads((prev) =>
        prev.map((d) =>
          d.id === p.id
            ? { ...d, downloaded_size: p.downloaded_size, total_size: p.total_size, speed: p.speed, status: p.status, segments: p.connections || d.segments }
            : d
        )
      );
    });

    const unlistenPair = listen<{ extension_id: string }>("pair-request", (event) => {
      showToast(
        "info",
        t("settings.pair_pending") + ": " + event.payload.extension_id
      );
    });

    return () => {
      unlistenAdded.then((f) => f());
      unlistenProgress.then((f) => f());
      unlistenPair.then((f) => f());
      clearInterval(refreshTimer);
    };
  }, [fetchDownloads, showToast, t]);

  // Clipboard URL monitor
  useEffect(() => {
    if (!clipboardMonitor) return;
    const timer = setInterval(async () => {
      try {
        const text = (await navigator.clipboard.readText()).trim();
        if (!text || text === lastClipboard) return;
        if (!URL_RE.test(text)) return;
        setLastClipboard(text);
        setPrefilledUrl(text);
        setIsModalOpen(true);
        showToast("info", t("app.clipboard_url"));
      } catch {
        /* clipboard permission denied — ignore */
      }
    }, 2000);
    return () => clearInterval(timer);
  }, [clipboardMonitor, lastClipboard, showToast, t]);

  const categoryCounts: Record<string, number> = {
    "All Downloads": downloads.length,
    "Downloading": downloads.filter((d) => d.status === "Downloading" || d.status === "Merging").length,
    "Completed": downloads.filter((d) => d.status === "Completed").length,
    "Paused": downloads.filter((d) => d.status === "Paused").length,
    "Failed": downloads.filter((d) => d.status === "Failed").length,
    "Video": downloads.filter((d) => d.category === "Video").length,
    "Music": downloads.filter((d) => d.category === "Music").length,
    "Documents": downloads.filter((d) => d.category === "Document").length,
    "Compressed": downloads.filter((d) => d.category === "Archive").length,
    "Programs": downloads.filter((d) => d.category === "Program").length,
  };

  const handlePauseAll = async () => {
    const active = downloads.filter((d) => d.status === "Downloading" || d.status === "Merging");
    await Promise.all(active.map((d) => invoke("pause_download", { id: d.id }).catch(() => {})));
    fetchDownloads();
    showToast("info", t("app.paused_count", { count: active.length }));
  };

  const handleResumeAll = async () => {
    const paused = downloads.filter((d) => d.status === "Paused" || d.status === "Failed");
    await Promise.all(paused.map((d) => invoke("resume_download", { id: d.id }).catch(() => {})));
    fetchDownloads();
    showToast("info", t("app.resumed_count", { count: paused.length }));
  };

  return (
    <div className="app-shell">
      {showOnboarding && (
        <OnboardingWizard
          onComplete={() => setShowOnboarding(false)}
          onSkip={() => setShowOnboarding(false)}
        />
      )}

      <TitleBar />

      <div className="app-body">
        <Sidebar activeCategory={activeCategory} onSelectCategory={setActiveCategory} counts={categoryCounts} />

        <main className="main-area">
        <Toolbar
          onAddClick={() => setIsModalOpen(true)}
          searchQuery={searchQuery}
          onSearchChange={setSearchQuery}
          onPauseAll={handlePauseAll}
          onResumeAll={handleResumeAll}
          clipboardMonitor={clipboardMonitor}
          onToggleClipboard={() => setClipboardMonitor((v) => !v)}
        />

        <div className="content-row">
          <DownloadList
            downloads={downloads}
            category={activeCategory}
            searchQuery={searchQuery}
            selectedId={selectedDownload?.id ?? null}
            onSelectDownload={setSelectedDownload}
            onRefresh={fetchDownloads}
            showToast={showToast}
            onAddClick={() => setIsModalOpen(true)}
          />

          {selectedDownload && (
            <InspectorPanel
              download={selectedDownload}
              onClose={() => setSelectedDownload(null)}
              onRefresh={fetchDownloads}
              showToast={showToast}
            />
          )}
        </div>
        <StatusBar downloads={downloads} selected={selectedDownload} />
        </main>
      </div>

      {isModalOpen && (
        <NewDownloadModal
          onClose={() => {
            setIsModalOpen(false);
            setPrefilledUrl("");
          }}
          onSuccess={() => setActiveCategory("All Downloads")}
          onAdded={fetchDownloads}
          initialUrl={prefilledUrl}
          showToast={showToast}
        />
      )}

      {toasts.length > 0 && (
        <div className="toast-stack" role="status" aria-live="polite">
          {toasts.map((tt) => (
            <div key={tt.id} className={`toast ${tt.kind}`}>
              {tt.kind === "success" ? <CheckCircle2 /> : tt.kind === "error" ? <AlertCircle /> : <Info />}
              <span>{tt.msg}</span>
            </div>
          ))}
        </div>
      )}
    </div>
  );
}

export default App;
