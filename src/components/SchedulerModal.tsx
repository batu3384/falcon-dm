import { useState, useEffect, useRef } from "react";
import { invoke } from "@tauri-apps/api/core";
import { useTranslation } from "react-i18next";
import { X } from "lucide-react";
import type { ScheduleModel } from "../types";
import { useModalA11y } from "../hooks/useModalA11y";

interface SchedulerModalProps {
  isOpen: boolean;
  onClose: () => void;
}

export function SchedulerModal({ isOpen, onClose }: SchedulerModalProps) {
  const { t } = useTranslation();
  const panelRef = useRef<HTMLDivElement>(null);
  useModalA11y(panelRef, onClose, isOpen);
  const [active, setActive] = useState(false);
  const [startTime, setStartTime] = useState("");
  const [stopTime, setStopTime] = useState("");
  const [saving, setSaving] = useState(false);
  const [error, setError] = useState("");

  useEffect(() => {
    if (!isOpen) return;
    invoke<ScheduleModel>("get_schedule")
      .then((s) => {
        setActive(s.active);
        setStartTime(s.start_time || "");
        setStopTime(s.stop_time || "");
      })
      .catch(() => {});
  }, [isOpen]);

  if (!isOpen) return null;

  const handleSave = async () => {
    setSaving(true);
    setError("");
    try {
      await invoke("set_schedule", {
        startTime: startTime || null,
        stopTime: stopTime || null,
        active,
      });
      onClose();
    } catch (e) {
      console.error(e);
      setError(t("scheduler.save_failed"));
    } finally {
      setSaving(false);
    }
  };

  return (
    <div className="modal-overlay" onClick={onClose} role="presentation">
      <div ref={panelRef} className="modal-panel" style={{ width: 440 }} onClick={(e) => e.stopPropagation()} role="dialog" aria-modal="true" aria-labelledby="sched-title">
        <div className="modal-head">
          <h2 id="sched-title" className="modal-title">{t("scheduler.title")}</h2>
          <button type="button" onClick={onClose} className="icon-btn" aria-label={t("scheduler.cancel")}><X size={18} /></button>
        </div>

        <div className="modal-body">
          <div className="check-row">
            <input id="enable-scheduler" type="checkbox" checked={active} onChange={(e) => setActive(e.target.checked)} />
            <label htmlFor="enable-scheduler">{t("scheduler.enable")}</label>
          </div>

          <div className="field">
            <label className="field-label" htmlFor="sched-start">{t("scheduler.start_time")}</label>
            <input id="sched-start" type="time" value={startTime} onChange={(e) => setStartTime(e.target.value)} disabled={!active} className="field-input" />
          </div>

          <div className="field">
            <label className="field-label" htmlFor="sched-stop">{t("scheduler.stop_time")}</label>
            <input id="sched-stop" type="time" value={stopTime} onChange={(e) => setStopTime(e.target.value)} disabled={!active} className="field-input" />
          </div>
          {error && <p className="field-error">{error}</p>}
        </div>

        <div className="modal-foot">
          <button type="button" className="btn-secondary" onClick={onClose}>{t("scheduler.cancel")}</button>
          <button type="button" className="btn-primary" onClick={handleSave} disabled={saving}>{saving ? t("scheduler.saving") : t("scheduler.save")}</button>
        </div>
      </div>
    </div>
  );
}
