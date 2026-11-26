import { useId, useState } from 'react';
import { useTranslation } from 'react-i18next';
import { Sheet } from './Sheet';

interface ConfirmDialogProps {
  message: string;
  detail?: string;
  confirmLabel?: string;
  cancelLabel?: string;
  danger?: boolean;
  deleteFileOption?: boolean;
  onConfirm: (deleteFromDisk?: boolean) => void;
  onCancel: () => void;
}

export function ConfirmDialog({
  message,
  detail,
  confirmLabel,
  cancelLabel,
  danger = true,
  deleteFileOption = false,
  onConfirm,
  onCancel,
}: ConfirmDialogProps) {
  const { t } = useTranslation();
  const titleId = useId();
  const detailId = useId();
  const checkId = useId();
  const hintId = useId();
  const [deleteFromDisk, setDeleteFromDisk] = useState(false);

  return (
    <Sheet
      size="small"
      role="alertdialog"
      labelledBy={titleId}
      describedBy={detail ? detailId : undefined}
      onClose={onCancel}
    >
      <div className="alert">
        <img src="/icon.png" alt="" />
        <h2 id={titleId}>{message}</h2>
        {detail && <p id={detailId}>{detail}</p>}
        {deleteFileOption && (
          <div className="check">
            <input
              id={checkId}
              type="checkbox"
              checked={deleteFromDisk}
              onChange={(e) => setDeleteFromDisk(e.target.checked)}
              aria-describedby={hintId}
            />
            <label htmlFor={checkId}>
              {t('confirm.delete_from_disk')}
              <span id={hintId} className="hint">
                {t('confirm.delete_from_disk_hint')}
              </span>
            </label>
          </div>
        )}
      </div>
      <div className="alert-foot">
        <button type="button" className="btn" data-modal-cancel onClick={onCancel}>
          {cancelLabel || t('common.cancel')}
        </button>
        <button
          type="button"
          className={`btn ${danger ? 'destructive' : 'primary'}`}
          onClick={() => onConfirm(deleteFileOption ? deleteFromDisk : undefined)}
        >
          {confirmLabel || t('common.remove')}
        </button>
      </div>
    </Sheet>
  );
}
