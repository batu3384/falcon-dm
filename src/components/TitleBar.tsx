import { useCallback } from "react";
import { getCurrentWindow } from "@tauri-apps/api/window";
import { useTranslation } from "react-i18next";

export default function TitleBar() {
  const { t } = useTranslation();

  const startDrag = useCallback(async (e: React.MouseEvent) => {
    if (e.button !== 0) return;
    const target = e.target as HTMLElement;
    if (target.closest("[data-tauri-drag-region='false']")) return;
    try {
      await getCurrentWindow().startDragging();
    } catch {
      /* browser preview */
    }
  }, []);

  return (
    <header
      className="titlebar"
      data-tauri-drag-region
      onMouseDown={startDrag}
      aria-label={t("app.window_title")}
    >
      <span className="titlebar-title">{t("sidebar.title")}</span>
    </header>
  );
}
