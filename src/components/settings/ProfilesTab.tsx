import { Plus, Trash2 } from 'lucide-react';
import { useTranslation } from 'react-i18next';
import type { SettingsModel } from '../../types';

type Props = {
  settings: SettingsModel;
  setSettings: (next: SettingsModel | ((previous: SettingsModel) => SettingsModel)) => void;
};

export function ProfilesTab({ settings, setSettings }: Props) {
  const { t } = useTranslation();
  return (
    <>
      <p className="field-hint">{t('settings.profiles_hint')}</p>
      <p className="field-hint">{t('settings.profile_host_hint')}</p>
      {(settings.download_profiles || []).map((p, i) => (
        <div className="profile-card" key={i}>
          <div className="profile-card-head">
            <span className="profile-card-title">{p.name || t('settings.profile_untitled')}</span>
            <button
              type="button"
              className="icon-btn danger"
              aria-label={t('settings.profile_remove')}
              onClick={() =>
                setSettings((prev) => ({
                  ...prev,
                  download_profiles: (prev.download_profiles || []).filter((_, idx) => idx !== i),
                }))
              }
            >
              <Trash2 size={14} />
            </button>
          </div>
          <div className="profile-fields">
            <input
              className="field-input"
              placeholder={t('settings.profile_name')}
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
        onClick={() =>
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
          }))
        }
      >
        <Plus size={14} /> {t('settings.profile_add')}
      </button>
    </>
  );
}
