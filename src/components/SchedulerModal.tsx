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
  const [hydrated, setHydrated] = useState(false);
  const [loadNonce, setLoadNonce] = useState(0);
  const [confirmClose, setConfirmClose] = useState(false);
  const dirtyRef = useRef(false);
  const requestClose = useCallback(() => {
    if (saving) return;
    if (dirtyRef.current) setConfirmClose(true);
    else onClose();
  }, [onClose, saving]);
  useModalA11y(panelRef, requestClose, isOpen);

  useEffect(() => {
    if (!isOpen) {
      dirtyRef.current = false;
      setHydrated(false);
      setError('');
      setConfirmClose(false);
      return;
    }
    let cancelled = false;
    api
      .getSchedule()
      .then((s) => {
        if (cancelled) return;
        setError('');
        setActive(s.active);
        setStartTime(s.start_time || '');
        setStopTime(s.stop_time || '');
        dirtyRef.current = false;
        setHydrated(true);
      })
      .catch(() => {
        if (cancelled) return;
        setError(t('scheduler.load_failed'));
      });
    return () => {
      cancelled = true;
    };
  }, [isOpen, t, loadNonce]);

  if (!isOpen) return null;

  const handleSave = async () => {
    if (!hydrated || saving) return;
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
            <X size={18} aria-hidden={true} />
          </button>
        </div>

        <div className="modal-body" {...(!hydrated ? { inert: true } : {})}>
          <div className="check-row">
            <input
              id="enable-scheduler"
              type="checkbox"
              checked={active}
              onChange={(e) => {
                dirtyRef.current = true;
                setActive(e.target.checked);
              }}
              aria-describedby="sched-scope-hint"
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
              aria-invalid={error ? true : undefined}
              aria-describedby={error ? 'sched-error sched-scope-hint' : 'sched-scope-hint'}
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
              aria-invalid={error ? true : undefined}
              aria-describedby={error ? 'sched-error sched-scope-hint' : 'sched-scope-hint'}
            />
          </div>
          <p id="sched-scope-hint" className="field-hint">
            {t('scheduler.scope_hint')}
          </p>
          {error && hydrated && (
            <p id="sched-error" className="field-error" role="alert">
              {error}
            </p>
          )}
        </div>

        <div className="modal-foot">
          {error && !hydrated && (
            <p id="sched-error" className="field-error" role="alert">
              {error}
            </p>
          )}
          <button type="button" className="btn-secondary" data-modal-cancel onClick={requestClose}>
            {t('scheduler.cancel')}
          </button>
          {error && !hydrated ? (
            <button
              type="button"
              className="btn-primary"
              onClick={() => {
                setError('');
                setLoadNonce((n) => n + 1);
              }}
            >
              {t('scheduler.retry')}
            </button>
          ) : (
            <button
              type="button"
              className="btn-primary"
              onClick={handleSave}
              disabled={saving || !hydrated}
            >
              {saving ? t('scheduler.saving') : t('scheduler.save')}
            </button>
          )}
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
