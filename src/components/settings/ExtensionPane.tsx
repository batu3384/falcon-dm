import { useState } from 'react';
import { useTranslation } from 'react-i18next';
import { Puzzle } from 'lucide-react';
import * as api from '../../api/commands';
import { useAppStore } from '../../store/app';
import { useToastStore } from '../../store/toast';
import { ConfirmDialog } from '../ConfirmDialog';

function shortId(id: string): string {
  return `${id.slice(0, 6)}…${id.slice(-4)}`;
}

export function ExtensionPane() {
  const { t } = useTranslation();
  const extension = useAppStore((s) => s.extension);
  const refreshExtension = useAppStore((s) => s.refreshExtension);
  const showToast = useToastStore((s) => s.showToast);
  const [revoking, setRevoking] = useState<string | null>(null);
  const approved = extension?.approved_extension_ids ?? [];
  const pending = extension?.pending ?? [];

  const run = async (fn: () => Promise<void>, success: string) => {
    try {
      await fn();
      showToast('success', success);
    } catch (e) {
      showToast('error', api.extractTauriError(e));
    } finally {
      void refreshExtension();
    }
  };

  return (
    <>
      <div className="status-card">
        <Puzzle size={20} color="var(--accent)" aria-hidden={true} />
        <span className="grow">{t('extension.intro')}</span>
      </div>

      {pending.length > 0 && (
        <section className="field">
          <span className="field-label">{t('extension.pending')}</span>
          <div className="list-box">
            {pending.map((request) => (
              <div className="list-box-row" key={request.extension_id}>
                <div className="grow">
                  <div className="title">
                    {t('extension.code')}{' '}
                    <span className="mono num">
                      {request.code.slice(0, 3)} {request.code.slice(3)}
                    </span>
                  </div>
                  <div className="sub mono" title={request.extension_id}>
                    {request.extension_id}
                  </div>
                </div>
                <button
                  type="button"
                  className="btn small"
                  onClick={() =>
                    void run(
                      () => api.rejectExtensionPair(request.extension_id),
                      t('extension.rejected'),
                    )
                  }
                >
                  {t('extension.reject')}
                </button>
                <button
                  type="button"
                  className="btn small primary"
                  onClick={() =>
                    void run(
                      () => api.approveExtensionPair(request.extension_id),
                      t('extension.approved'),
                    )
                  }
                >
                  {t('extension.approve')}
                </button>
              </div>
            ))}
          </div>
        </section>
      )}

      <section className="field">
        <span className="field-label">{t('extension.connected')}</span>
        <div className="list-box">
          {approved.length === 0 ? (
            <div className="list-box-empty">{t('extension.none')}</div>
          ) : (
            approved.map((id) => (
              <div className="list-box-row" key={id}>
                <span className="dot on" aria-hidden="true" />
                <div className="grow">
                  <div className="title">{t('extension.browser_extension')}</div>
                  <div className="sub mono" title={id}>
                    {shortId(id)}
                  </div>
                </div>
                <button type="button" className="btn small" onClick={() => setRevoking(id)}>
                  {t('extension.revoke')}
                </button>
              </div>
            ))
          )}
        </div>
      </section>

      <section className="field">
        <span className="field-label">{t('extension.setup')}</span>
        <ol className="steps">
          <li>{t('extension.step_open')}</li>
          <li>{t('extension.step_load')}</li>
          <li>{t('extension.step_pair')}</li>
        </ol>
      </section>

      {revoking && (
        <ConfirmDialog
          message={t('extension.revoke_title')}
          detail={t('extension.revoke_detail')}
          confirmLabel={t('extension.revoke')}
          onConfirm={() => {
            const id = revoking;
            setRevoking(null);
            void run(() => api.revokeExtension(id), t('extension.revoked'));
          }}
          onCancel={() => setRevoking(null)}
        />
      )}
    </>
  );
}
