import { useState, useEffect } from "react";
import { useTranslation } from "react-i18next";
import { invoke } from "@tauri-apps/api/core";
import { Zap, CheckCircle2, ChevronRight, ChevronLeft, Puzzle, Copy } from "lucide-react";

interface OnboardingProps {
  onComplete: () => void;
  onSkip?: () => void;
}

export const OnboardingWizard = ({ onComplete, onSkip }: OnboardingProps) => {
  const { t } = useTranslation();
  const [step, setStep] = useState(1);
  const [token, setToken] = useState("");
  const [copied, setCopied] = useState(false);

  useEffect(() => {
    invoke<string>("get_api_token").then(setToken).catch(() => {});
  }, []);

  const finish = () => {
    localStorage.setItem("onboarding_complete", "true");
    onComplete();
  };

  const skip = () => {
    localStorage.setItem("onboarding_complete", "true");
    onSkip?.();
    onComplete();
  };

  const copyToken = () => {
    navigator.clipboard.writeText(token).then(() => {
      setCopied(true);
      setTimeout(() => setCopied(false), 1500);
    });
  };

  const instructions = [
    t("onboarding.step2_instruction1"),
    t("onboarding.step2_instruction2"),
    t("onboarding.step2_instruction3"),
    t("onboarding.step2_instruction4"),
  ];

  return (
    <div className="wizard-overlay" role="dialog" aria-modal="true" aria-labelledby="wizard-title">
      <div className="wizard-panel">
        <div className="wizard-steps">
          <div className={step >= 1 ? "active" : ""} />
          <div className={step >= 2 ? "active" : ""} />
        </div>

        <div className="wizard-content">
          {step === 1 ? (
            <div className={`wizard-icon accent`}><Zap strokeWidth={1.6} /></div>
          ) : (
            <div className="wizard-icon success"><Puzzle strokeWidth={1.6} /></div>
          )}

          <h2 id="wizard-title" className="wizard-h">{step === 1 ? t("onboarding.step1_title") : t("onboarding.step2_title")}</h2>
          <p className="wizard-p">{step === 1 ? t("onboarding.step1_desc") : t("onboarding.step2_desc")}</p>

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
              <div className="field" style={{ marginTop: 12 }}>
                <label className="field-label">{t("onboarding.api_token")}</label>
                <div className="input-action">
                  <input className="field-input" readOnly value={token} />
                  <button type="button" className="btn-secondary" onClick={copyToken}>
                    <Copy size={14} /> {copied ? t("onboarding.copied") : t("onboarding.copy")}
                  </button>
                </div>
              </div>
            </>
          )}
        </div>

        <div className="wizard-foot">
          {step > 1 ? (
            <button type="button" onClick={() => setStep(step - 1)} className="btn-ghost">
              <ChevronLeft size={15} /> {t("onboarding.back")}
            </button>
          ) : (
            <button type="button" onClick={skip} className="btn-ghost">{t("onboarding.skip")}</button>
          )}
          {step < 2 ? (
            <button type="button" onClick={() => setStep(step + 1)} className="btn-primary">
              {t("onboarding.next")} <ChevronRight size={15} />
            </button>
          ) : (
            <button type="button" onClick={finish} className="btn-primary">
              <CheckCircle2 size={15} /> {t("onboarding.get_started")}
            </button>
          )}
        </div>
      </div>
    </div>
  );
};
