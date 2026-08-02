import { useRef } from 'react';
import { useTranslation } from 'react-i18next';
import { useModalA11y } from '../hooks/useModalA11y';

interface ConfirmDialogProps {
  message: string;
  confirmLabel?: string;
  cancelLabel?: string;
  danger?: boolean;
  onConfirm: () => void;
  onCancel: () => void;
}

export function ConfirmDialog({
  message,
  confirmLabel,
  cancelLabel,
  danger = true,
  onConfirm,
  onCancel,
}: ConfirmDialogProps) {
  const { t } = useTranslation();
  const panelRef = useRef<HTMLDivElement>(null);
  useModalA11y(panelRef, onCancel);

  return (
    <div className="modal-overlay" onClick={onCancel} role="presentation">
      <div
        ref={panelRef}
        className="modal-panel"
        style={{ width: 380 }}
        onClick={(e) => e.stopPropagation()}
        role="alertdialog"
        aria-modal="true"
        aria-describedby="confirm-msg"
      >
        <div className="modal-body">
          <p
            id="confirm-msg"
            className="field-hint"
            style={{ margin: 0, color: 'inherit', fontSize: 14 }}
          >
            {message}
          </p>
        </div>
        <div className="modal-foot">
          <button type="button" className="btn-secondary" onClick={onCancel}>
            {cancelLabel || t('settings.cancel')}
          </button>
          <button
            type="button"
            className="btn-primary"
            style={danger ? { background: 'var(--danger)' } : undefined}
            onClick={onConfirm}
          >
            {confirmLabel || t('inspector.remove')}
          </button>
        </div>
      </div>
    </div>
  );
}
