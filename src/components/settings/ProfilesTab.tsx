import { useRef } from 'react';
import { Plus, Trash2 } from 'lucide-react';
import { useTranslation } from 'react-i18next';
import type { SettingsModel } from '../../types';

type Props = {
  settings: SettingsModel;
  setSettings: (next: SettingsModel | ((previous: SettingsModel) => SettingsModel)) => void;
};

let nextProfileUiKey = 0;
function allocProfileUiKey() {
  nextProfileUiKey += 1;
  return `profile-${nextProfileUiKey}`;
}

export function ProfilesTab({ settings, setSettings }: Props) {
  const { t } = useTranslation();
  const keysRef = useRef<string[]>([]);
  const profiles = settings.download_profiles || [];
  if (keysRef.current.length < profiles.length) {
    keysRef.current = [
      ...keysRef.current,
      ...Array.from({ length: profiles.length - keysRef.current.length }, allocProfileUiKey),
    ];
  }
  return (
    <>
      <p className="field-hint" id="settings-profiles-hint">
        {t('settings.profiles_hint')}
      </p>
      <p className="field-hint" id="settings-profile-host-hint">
        {t('settings.profile_host_hint')}
      </p>
      {profiles.map((p, i) => (
        <div className="profile-card" key={keysRef.current[i]}>
          <div className="profile-card-head">
            <span className="profile-card-title">{p.name || t('settings.profile_untitled')}</span>
            <button
              type="button"
              className="icon-btn danger"
              aria-label={t('settings.profile_remove')}
              onClick={() => {
                keysRef.current = keysRef.current.filter((_, idx) => idx !== i);
                setSettings((prev) => ({
                  ...prev,
                  download_profiles: (prev.download_profiles || []).filter((_, idx) => idx !== i),
                }));
              }}
            >
              <Trash2 size={14} aria-hidden={true} />
            </button>
          </div>
          <div className="profile-fields">
            <input
              className="field-input"
              placeholder={t('settings.profile_name')}
              aria-label={t('settings.profile_name')}
              value={p.name}
              onChange={(e) => {
                const v = e.target.value;
                setSettings((prev) => ({
                  ...prev,
                  download_profiles: (prev.download_profiles || []).map((q, idx) =>
                    idx === i ? { ...q, name: v } : q,
                  ),
                }));
              }}
            />
            <input
              className="field-input"
              placeholder={t('settings.profile_url_pattern')}
              aria-label={t('settings.profile_url_pattern')}
              aria-describedby="settings-profile-host-hint"
              value={p.url_pattern}
              onChange={(e) => {
                const v = e.target.value;
                setSettings((prev) => ({
                  ...prev,
                  download_profiles: (prev.download_profiles || []).map((q, idx) =>
                    idx === i ? { ...q, url_pattern: v } : q,
                  ),
                }));
              }}
            />
            <input
              className="field-input"
              placeholder={t('settings.profile_save_subdir')}
              aria-label={t('settings.profile_save_subdir')}
              value={p.save_subdir || ''}
              onChange={(e) => {
                const v = e.target.value;
                setSettings((prev) => ({
                  ...prev,
                  download_profiles: (prev.download_profiles || []).map((q, idx) =>
                    idx === i ? { ...q, save_subdir: v || null } : q,
                  ),
                }));
              }}
            />
            <input
              className="field-input"
              placeholder={t('settings.profile_user_agent')}
              aria-label={t('settings.profile_user_agent')}
              value={p.user_agent || ''}
              onChange={(e) => {
                const v = e.target.value;
                setSettings((prev) => ({
                  ...prev,
                  download_profiles: (prev.download_profiles || []).map((q, idx) =>
                    idx === i ? { ...q, user_agent: v || null } : q,
                  ),
                }));
              }}
            />
            <input
              className="field-input"
              placeholder={t('settings.profile_referrer')}
              aria-label={t('settings.profile_referrer')}
              value={p.referrer || ''}
              onChange={(e) => {
                const v = e.target.value;
                setSettings((prev) => ({
                  ...prev,
                  download_profiles: (prev.download_profiles || []).map((q, idx) =>
                    idx === i ? { ...q, referrer: v || null } : q,
                  ),
                }));
              }}
            />
            <input
              className="field-input"
              type="password"
              autoComplete="off"
              placeholder={t('settings.profile_cookies_keep')}
              aria-label={t('settings.profile_cookies')}
              value={p.cookies || ''}
              onChange={(e) => {
                const v = e.target.value;
                setSettings((prev) => ({
                  ...prev,
                  download_profiles: (prev.download_profiles || []).map((q, idx) =>
                    idx === i ? { ...q, cookies: v || null } : q,
                  ),
                }));
              }}
            />
          </div>
        </div>
      ))}
      <button
        type="button"
        className="btn-secondary"
        onClick={() => {
          keysRef.current = [...keysRef.current, allocProfileUiKey()];
          setSettings((prev) => ({
            ...prev,
            download_profiles: [
              ...(prev.download_profiles || []),
              {
                name: '',
                url_pattern: '',
                user_agent: null,
                referrer: null,
                cookies: null,
                save_subdir: null,
              },
            ],
          }));
        }}
      >
        <Plus size={14} aria-hidden={true} /> {t('settings.profile_add')}
      </button>
    </>
  );
}
