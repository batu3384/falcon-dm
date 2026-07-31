import { useTranslation } from "react-i18next";
import type { DownloadModel } from "../types";
import { formatBytes } from "../types";

interface StatusBarProps {
  downloads: DownloadModel[];
  selected: DownloadModel | null;
}

export default function StatusBar({ downloads, selected }: StatusBarProps) {
  const { t } = useTranslation();
  const active = downloads.filter((d) => d.status === "Downloading" || d.status === "Merging");
  const queued = downloads.filter((d) => d.status === "Queued");
  const totalSpeed = active.reduce((s, d) => s + (d.speed || 0), 0);

  return (
    <footer className="status-bar no-drag" role="status" aria-live="polite">
      <span>
        {t("statusBar.speed")}: <strong className="mono">{formatBytes(totalSpeed)}/s</strong>
      </span>
      <span className="status-sep" aria-hidden />
      <span>
        {t("statusBar.active")}: <strong>{active.length}</strong>
      </span>
      <span className="status-sep" aria-hidden />
      <span>
        {t("statusBar.queued")}: <strong>{queued.length}</strong>
      </span>
      <span className="status-sep" aria-hidden />
      <span>
        {t("statusBar.total")}: <strong>{downloads.length}</strong>
      </span>
      {selected && (
        <>
          <span className="status-sep" aria-hidden />
          <span className="status-selected" title={selected.filename}>
            {selected.filename}
          </span>
        </>
      )}
    </footer>
  );
}
