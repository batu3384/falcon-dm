import { useEffect, useState } from 'react';
import { useTranslation } from 'react-i18next';
import { Film, Gauge, Puzzle } from 'lucide-react';
import { Sheet } from './Sheet';
import { useAppStore } from '../store/app';

export const ONBOARDING_KEY = 'onboarding_complete';

export function OnboardingWizard({ onClose }: { onClose: () => void }) {
  const { t } = useTranslation();
  const [step, setStep] = useState(0);
  const extension = useAppStore((s) => s.extension);
  const refreshExtension = useAppStore((s) => s.refreshExtension);
  const paired = (extension?.approved_extension_ids.length ?? 0) > 0;
  const pending = (extension?.pending.length ?? 0) > 0;

  useEffect(() => {
    void refreshExtension();
  }, [refreshExtension]);

  const finish = () => {
    localStorage.setItem(ONBOARDING_KEY, 'true');
    onClose();
  };

  const steps = 2;
  return (
    <Sheet
      label={t('onboarding.title')}
      onClose={finish}
      footer={
        <>
          <div className="step-dots grow" aria-hidden="true">
            {Array.from({ length: steps }, (_, i) => (
              <span key={i} className={i === step ? 'on' : ''} />
            ))}
          </div>
          {step === 0 ? (
            <button type="button" className="btn" onClick={finish}>
              {t('onboarding.skip')}
            </button>
          ) : (
            <button type="button" className="btn" onClick={() => setStep(step - 1)}>
              {t('onboarding.back')}
            </button>
          )}
          {step < steps - 1 ? (
            <button
              type="button"
              className="btn primary"
              onClick={() => setStep(step + 1)}
              data-modal-initial-focus
            >
              {t('onboarding.next')}
            </button>
          ) : (
            <button type="button" className="btn primary" onClick={finish} data-modal-initial-focus>
              {t('onboarding.done')}
            </button>
          )}
        </>
      }
    >
      <div className="onboard" aria-live="polite">
        {step === 0 && (
          <>
            <div className="onboard-hero">
              <img src="/icon.png" alt="" />
              <h2>{t('onboarding.title')}</h2>
              <p>{t('onboarding.subtitle')}</p>
            </div>
            <ul className="features">
              <li>
                <Gauge size={22} strokeWidth={1.6} aria-hidden={true} />
                <div>
                  <strong>{t('onboarding.f_fast')}</strong>
                  <span>{t('onboarding.f_fast_desc')}</span>
                </div>
              </li>
              <li>
                <Film size={22} strokeWidth={1.6} aria-hidden={true} />
                <div>
                  <strong>{t('onboarding.f_video')}</strong>
                  <span>{t('onboarding.f_video_desc')}</span>
                </div>
              </li>
              <li>
                <Puzzle size={22} strokeWidth={1.6} aria-hidden={true} />
                <div>
                  <strong>{t('onboarding.f_browser')}</strong>
                  <span>{t('onboarding.f_browser_desc')}</span>
                </div>
              </li>
            </ul>
          </>
        )}
        {step === 1 && (
          <>
            <div className="onboard-hero">
              <h2>{t('onboarding.ext_title')}</h2>
              <p>{t('onboarding.ext_desc')}</p>
            </div>
            <ol className="steps">
              <li>{t('extension.step_open')}</li>
              <li>{t('extension.step_load')}</li>
              <li>{t('extension.step_pair')}</li>
            </ol>
            <div className="status-card" role="status">
              <span
                className={`dot${paired ? ' on' : pending ? ' wait' : ''}`}
                aria-hidden="true"
              />
              <span className="grow">
                {paired
                  ? t('onboarding.ext_paired')
                  : pending
                    ? t('onboarding.ext_pending')
                    : t('onboarding.ext_waiting')}
              </span>
            </div>
          </>
        )}
      </div>
    </Sheet>
  );
}
