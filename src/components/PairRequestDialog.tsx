import { useId, useState } from 'react';
import { useTranslation } from 'react-i18next';
import { Sheet } from './Sheet';
import * as api from '../api/commands';
import { useAppStore } from '../store/app';
import { useToastStore } from '../store/toast';
import type { PairRequest } from '../types';

/** An extension asks to connect: the user compares the code it shows. */
export function PairRequestDialog({ request }: { request: PairRequest }) {
  const { t } = useTranslation();
  const dismiss = useAppStore((s) => s.dismissPairPrompt);
  const showToast = useToastStore((s) => s.showToast);
  const [busy, setBusy] = useState(false);
  const titleId = useId();

  const answer = async (approve: boolean) => {
    setBusy(true);
    try {
      if (approve) await api.approveExtensionPair(request.extension_id);
      else await api.rejectExtensionPair(request.extension_id);
      showToast(
        approve ? 'success' : 'info',
        approve ? t('extension.approved') : t('extension.rejected'),
      );
    } catch (e) {
      showToast('error', api.extractTauriError(e));
    } finally {
      setBusy(false);
      dismiss();
    }
  };

  return (
    // Escape only hides the prompt; the request stays answerable in Settings.
    <Sheet size="small" role="alertdialog" labelledBy={titleId} onClose={dismiss}>
      <div className="alert">
        <img src="/icon.png" alt="" />
        <h2 id={titleId}>{t('pair.title')}</h2>
        <p>{t('pair.body')}</p>
        <div
          className="pair-code selectable"
          aria-label={t('pair.code_label', { code: request.code })}
        >
          {request.code.slice(0, 3)} {request.code.slice(3)}
        </div>
        <p>{t('pair.warning')}</p>
      </div>
      <div className="alert-foot">
        <button
          type="button"
          className="btn"
          data-modal-cancel
          disabled={busy}
          onClick={() => void answer(false)}
        >
          {t('extension.reject')}
        </button>
        <button
          type="button"
          className="btn primary"
          disabled={busy}
          onClick={() => void answer(true)}
        >
          {t('pair.approve')}
        </button>
      </div>
    </Sheet>
  );
}
