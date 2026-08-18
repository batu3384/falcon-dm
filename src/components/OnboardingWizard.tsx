import { useState, useEffect, useRef, useCallback } from 'react';
import { useTranslation } from 'react-i18next';
import { invoke } from '@tauri-apps/api/core';
import { Zap, CheckCircle2, ChevronRight, ChevronLeft, Puzzle, ShieldCheck } from 'lucide-react';
import { useModalA11y } from '../hooks/useModalA11y';
import { onPairRequest } from '../api/events';
import * as api from '../api/commands';
import { useToastStore } from '../store/toast';

interface ExtensionStatus {
  has_token: boolean;
  approved_extension_ids: string[];
  pending_pair_id: string | null;
}

interface OnboardingProps {
  onComplete: () => void;
  onSkip?: () => void;
}

export const OnboardingWizard = ({ onComplete, onSkip }: OnboardingProps) => {
  const { t } = useTranslation();
  const showToast = useToastStore((s) => s.showToast);
  const [step, setStep] = useState(1);
  const [status, setStatus] = useState<ExtensionStatus | null>(null);
  const [extensionId, setExtensionId] = useState('');
  const [installingHost, setInstallingHost] = useState(false);
  const panelRef = useRef<HTMLDivElement>(null);
  const handleClose = useCallback(() => {
    localStorage.setItem('onboarding_complete', 'true');
    onSkip?.();
    onComplete();
  }, [onSkip, onComplete]);
  useModalA11y(panelRef, handleClose);

  const refreshStatus = useCallback(async () => {
    try {
      setStatus(await invoke<ExtensionStatus>('get_extension_status'));
    } catch {
      /* token fetch is best-effort; the pairing flow still works via Settings */
    }
  }, []);

  useEffect(() => {
    refreshStatus();
    // Live-update when the user approves the extension while the wizard is open.
    const unlisten = onPairRequest(() => refreshStatus());
    return () => {
      unlisten.then((fn) => fn());
    };
  }, [refreshStatus]);

  const finish = () => {
    localStorage.setItem('onboarding_complete', 'true');
    onComplete();
  };

  const skip = () => {
    handleClose();
  };

  const instructions = [
    t('onboarding.step2_instruction1'),
    t('onboarding.step2_instruction2'),
    t('onboarding.step2_instruction3'),
    t('onboarding.step2_instruction4'),
  ];

  const paired = (status?.approved_extension_ids?.length ?? 0) > 0;
  const pending = !!status?.pending_pair_id;
  const suggestedExtensionId = status?.pending_pair_id || status?.approved_extension_ids?.[0] || '';

  const handleInstallNativeHost = async () => {
    const chromeId = (extensionId || suggestedExtensionId).trim();
    if (chromeId.length !== 32) {
      showToast('error', t('onboarding.native_host_invalid_id'));
      return;
    }
    setInstallingHost(true);
    try {
      await api.installNativeHostManifests(chromeId);
      showToast('success', t('onboarding.native_host_installed'));
    } catch (e) {
      console.error(e);
      showToast('error', t('onboarding.native_host_failed'));
    } finally {
      setInstallingHost(false);
    }
  };

  return (
    <div className="wizard-overlay" onClick={skip} role="presentation">
      <div
        ref={panelRef}
        className="wizard-panel"
        onClick={(e) => e.stopPropagation()}
        role="dialog"
        aria-modal="true"
        aria-labelledby="wizard-title"
      >
        <div className="wizard-steps">
          <div className={step >= 1 ? 'active' : ''} />
          <div className={step >= 2 ? 'active' : ''} />
        </div>

        <div className="wizard-content">
          {step === 1 ? (
            <div className={`wizard-icon accent`}>
              <Zap strokeWidth={1.6} />
            </div>
          ) : (
            <div className="wizard-icon success">
              <Puzzle strokeWidth={1.6} />
            </div>
          )}

          <h2 id="wizard-title" className="wizard-h">
            {step === 1 ? t('onboarding.step1_title') : t('onboarding.step2_title')}
          </h2>
          <p className="wizard-p">
            {step === 1 ? t('onboarding.step1_desc') : t('onboarding.step2_desc')}
          </p>

          {step === 2 && (
            <>
              <div className="wizard-steps-box">
                {instructions.map((text, i) => (
                  <div key={i} className="wizard-step-item">
                    <span className="wizard-step-num">{i + 1}</span>
                    <span>{text}</span>
                  </div>
                ))}
              </div>
              {/* ponytail: pair status replaces the raw API token. The secret
                  token never reaches the frontend now — it only leaves the app
                  via the authenticated /api/pair HTTP flow after the user
                  explicitly approves a specific extension ID. */}
              <div className="field" style={{ marginTop: 12 }}>
                <div
                  className="pair-status"
                  data-state={paired ? 'paired' : pending ? 'pending' : 'waiting'}
                >
                  {paired ? (
                    <>
                      <ShieldCheck size={16} />
                      <span>{t('onboarding.pair_paired')}</span>
                    </>
                  ) : pending ? (
                    <>
                      <Puzzle size={16} />
                      <span>{t('onboarding.pair_pending')}</span>
                    </>
                  ) : (
                    <>
                      <Zap size={16} />
                      <span>{t('onboarding.pair_waiting')}</span>
                    </>
                  )}
                </div>
              </div>
              <div className="field" style={{ marginTop: 12 }}>
                <label className="field-label" htmlFor="onboard-ext-id">
                  {t('onboarding.native_host_id')}
                </label>
                <input
                  id="onboard-ext-id"
                  className="field-input mono"
                  placeholder={suggestedExtensionId || 'abcdefghijklmnopqrstuvwxyzabcdef'}
                  value={extensionId}
                  onChange={(e) => setExtensionId(e.target.value)}
                />
                <p className="field-hint">{t('onboarding.native_host_hint')}</p>
                <button
                  type="button"
                  className="btn-secondary"
                  style={{ marginTop: 8 }}
                  onClick={handleInstallNativeHost}
                  disabled={installingHost}
                >
                  {installingHost ? t('onboarding.native_host_installing') : t('onboarding.native_host_install')}
                </button>
              </div>
            </>
          )}
        </div>

        <div className="wizard-foot">
          {step > 1 ? (
            <button type="button" onClick={() => setStep(step - 1)} className="btn-ghost">
              <ChevronLeft size={15} /> {t('onboarding.back')}
            </button>
          ) : (
            <button type="button" onClick={skip} className="btn-ghost">
              {t('onboarding.skip')}
            </button>
          )}
          {step < 2 ? (
            <button type="button" onClick={() => setStep(step + 1)} className="btn-primary">
              {t('onboarding.next')} <ChevronRight size={15} />
            </button>
          ) : (
            <button type="button" onClick={finish} className="btn-primary">
              <CheckCircle2 size={15} /> {t('onboarding.get_started')}
            </button>
          )}
        </div>
      </div>
    </div>
  );
};
