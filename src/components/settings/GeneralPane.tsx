import { useState } from 'react';
import { useTranslation } from 'react-i18next';
import { openUrl } from '@tauri-apps/plugin-opener';
import * as api from '../../api/commands';
import { isFalconGithubReleaseUrl } from '../../lib/publicHttpUrl';
import { useToastStore } from '../../store/toast';
import { useUiStore } from '../../store/ui';
import { Check, FormRow } from './FormRow';
import type { PaneProps, Prefs } from './types';

export function GeneralPane({
  settings,
  update,
  prefs,
  setPrefs,
}: PaneProps & { prefs: Prefs; setPrefs: (patch: Partial<Prefs>) => void }) {
  const { t } = useTranslation();
  const showToast = useToastStore((s) => s.showToast);
  const setLogsOpen = useUiStore((s) => s.setLogsOpen);
  const setOnboardingOpen = useUiStore((s) => s.setOnboardingOpen);
  const [checking, setChecking] = useState(false);

  const checkUpdates = async () => {
    setChecking(true);
    try {
      const result = await api.checkForUpdates();
      if (!result.update_available) {
        showToast('success', t('settings.update_current', { version: result.current_version }));
      } else {
        const releaseUrl = result.release_url;
        const action =
          releaseUrl && isFalconGithubReleaseUrl(releaseUrl)
            ? {
                label: t('settings.update_download'),
                run: () => {
                  void openUrl(releaseUrl).catch(() =>
                    showToast('error', t('settings.update_failed')),
                  );
                },
              }
            : undefined;
        showToast(
          'info',
          t('settings.update_available', { version: result.latest_version ?? '' }),
          action,
        );
      }
    } catch {
      showToast('error', t('settings.update_failed'));
    } finally {
      setChecking(false);
    }
  };

  return (
    <>
      <FormRow label={t('settings.appearance')} htmlFor="set-theme">
        <select
          id="set-theme"
          className="select"
          value={settings.theme}
          onChange={(e) => update({ theme: e.target.value })}
        >
          <option value="system">{t('settings.theme_system')}</option>
          <option value="light">{t('settings.theme_light')}</option>
          <option value="dark">{t('settings.theme_dark')}</option>
        </select>
      </FormRow>
      <FormRow label={t('settings.language')} htmlFor="set-lang">
        <select
          id="set-lang"
          className="select"
          value={prefs.language}
          onChange={(e) => setPrefs({ language: e.target.value })}
        >
          <option value="system">{t('settings.language_system')}</option>
          <option value="en">English</option>
          <option value="tr">Türkçe</option>
        </select>
      </FormRow>
      <div className="form-sep" />
      <FormRow label={t('settings.clipboard')} plain>
        <Check
          id="set-clipboard"
          checked={prefs.clipboardMonitor}
          onChange={(clipboardMonitor) => setPrefs({ clipboardMonitor })}
          label={t('settings.clipboard_label')}
          hint={t('settings.clipboard_hint')}
        />
      </FormRow>
      <div className="form-sep" />
      <FormRow label={t('settings.updates')} plain>
        <div className="input-row">
          <button
            type="button"
            className="btn"
            onClick={() => void checkUpdates()}
            disabled={checking}
          >
            {checking ? t('settings.checking') : t('settings.check_updates')}
          </button>
        </div>
      </FormRow>
      <FormRow label={t('settings.troubleshooting')} plain>
        <div className="input-row">
          <button type="button" className="btn" onClick={() => setLogsOpen(true)}>
            {t('settings.show_logs')}
          </button>
          <button type="button" className="btn" onClick={() => setOnboardingOpen(true)}>
            {t('settings.show_welcome')}
          </button>
        </div>
      </FormRow>
    </>
  );
}
