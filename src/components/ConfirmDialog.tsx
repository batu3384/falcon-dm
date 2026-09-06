import { useRef } from 'react';
import { createPortal } from 'react-dom';
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

  return createPortal(
    <div
      className="modal-overlay modal-overlay-root"
      onClick={(e) => {
        e.stopPropagation();
        if (e.target === e.currentTarget) onCancel();
      }}
      role="presentation"
    >
      <div
        ref={panelRef}
        className="modal-panel modal-sm"
        onClick={(e) => e.stopPropagation()}
        role="alertdialog"
        aria-modal="true"
        aria-describedby="confirm-msg"
      >
        <div className="modal-body">
          <p id="confirm-msg" className="confirm-msg">
            {message}
          </p>
        </div>
        <div className="modal-foot">
          <button type="button" className="btn-secondary" data-modal-cancel onClick={onCancel}>
            {cancelLabel || t('settings.cancel')}
          </button>
          <button
            type="button"
            className={`btn-primary${danger ? ' danger' : ''}`}
            onClick={onConfirm}
          >
            {confirmLabel || t('inspector.remove')}
          </button>
        </div>
      </div>
    </div>,
    document.body,
  );
}
