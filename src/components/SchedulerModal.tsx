import { useState, useEffect, useRef, useCallback } from 'react';
import { useTranslation } from 'react-i18next';
import { X } from 'lucide-react';
import { useModalA11y } from '../hooks/useModalA11y';
import { ConfirmDialog } from './ConfirmDialog';
import * as api from '../api/commands';
import { isValidSchedule } from '../lib/scheduler';

interface SchedulerModalProps {
  isOpen: boolean;
  onClose: () => void;
}

export function SchedulerModal({ isOpen, onClose }: SchedulerModalProps) {
  const { t } = useTranslation();
  const panelRef = useRef<HTMLDivElement>(null);
  const [active, setActive] = useState(false);
  const [startTime, setStartTime] = useState('');
  const [stopTime, setStopTime] = useState('');
  const [saving, setSaving] = useState(false);
  const [error, setError] = useState('');
  const [confirmClose, setConfirmClose] = useState(false);
  const dirtyRef = useRef(false);
  const requestClose = useCallback(() => {
    if (saving) return;
    if (dirtyRef.current) setConfirmClose(true);
    else onClose();
  }, [onClose, saving]);
  useModalA11y(panelRef, requestClose, isOpen);

  useEffect(() => {
    if (!isOpen) return;
    api
      .getSchedule()
      .then((s) => {
        if (dirtyRef.current) return;
        setActive(s.active);
        setStartTime(s.start_time || '');
        setStopTime(s.stop_time || '');
      })
      .catch(() => {});
  }, [isOpen]);

  if (!isOpen) return null;

  const handleSave = async () => {
    setSaving(true);
    setError('');
    if (
      !isValidSchedule({
        start_time: startTime || null,
        stop_time: stopTime || null,
        active,
      })
    ) {
      setError(t('scheduler.invalid_time'));
      setSaving(false);
      return;
    }
    try {
      await api.setSchedule({
        startTime: startTime || null,
        stopTime: stopTime || null,
        active,
      });
      dirtyRef.current = false;
      onClose();
    } catch (e) {
      console.error(e);
      setError(t('scheduler.save_failed'));
    } finally {
      setSaving(false);
    }
  };

  return (
    <div className="modal-overlay" onClick={requestClose} role="presentation">
      <div
        ref={panelRef}
        className="modal-panel modal-sm"
        onClick={(e) => e.stopPropagation()}
        role="dialog"
        aria-modal="true"
        aria-labelledby="sched-title"
      >
        <div className="modal-head">
          <h2 id="sched-title" className="modal-title">
            {t('scheduler.title')}
          </h2>
          <button
            type="button"
            onClick={requestClose}
            className="icon-btn"
            data-modal-cancel
            aria-label={t('scheduler.cancel')}
          >
            <X size={18} />
          </button>
        </div>

        <div className="modal-body">
          <div className="check-row">
            <input
              id="enable-scheduler"
              type="checkbox"
              checked={active}
              onChange={(e) => {
                dirtyRef.current = true;
                setActive(e.target.checked);
              }}
            />
            <label htmlFor="enable-scheduler">{t('scheduler.enable')}</label>
          </div>

          <div className="field">
            <label className="field-label" htmlFor="sched-start">
              {t('scheduler.start_time')}
            </label>
            <input
              id="sched-start"
              type="time"
              value={startTime}
              onChange={(e) => {
                dirtyRef.current = true;
                setStartTime(e.target.value);
              }}
              disabled={!active}
              className="field-input"
            />
          </div>

          <div className="field">
            <label className="field-label" htmlFor="sched-stop">
              {t('scheduler.stop_time')}
            </label>
            <input
              id="sched-stop"
              type="time"
              value={stopTime}
              onChange={(e) => {
                dirtyRef.current = true;
                setStopTime(e.target.value);
              }}
              disabled={!active}
              className="field-input"
            />
          </div>
          {error && <p className="field-error">{error}</p>}
        </div>

        <div className="modal-foot">
          <button type="button" className="btn-secondary" data-modal-cancel onClick={requestClose}>
            {t('scheduler.cancel')}
          </button>
          <button type="button" className="btn-primary" onClick={handleSave} disabled={saving}>
            {saving ? t('scheduler.saving') : t('scheduler.save')}
          </button>
        </div>
      </div>
      {confirmClose && (
        <ConfirmDialog
          message={t('scheduler.confirm_close')}
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
