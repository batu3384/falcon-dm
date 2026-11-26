import { useTranslation } from 'react-i18next';
import { Plus, Trash2 } from 'lucide-react';
import type { DownloadProfile } from '../../types';
import type { PaneProps } from './types';

const EMPTY: DownloadProfile = {
  name: '',
  url_pattern: '',
  user_agent: null,
  referrer: null,
  cookies: null,
  save_subdir: null,
};

export function ProfilesPane({ settings, update }: PaneProps) {
  const { t } = useTranslation();
  const profiles = settings.download_profiles ?? [];

  const change = (index: number, patch: Partial<DownloadProfile>) =>
    update({
      download_profiles: profiles.map((profile, i) =>
        i === index ? { ...profile, ...patch } : profile,
      ),
    });

  const text = (value: string) => (value.trim() ? value : null);

  return (
    <>
      <p className="hint flush">{t('profiles.intro')}</p>
      {profiles.map((profile, index) => (
        // Controlled inputs: an index key only shifts values when a card is removed.
        <div className="profile" key={index}>
          <div className="profile-head">
            <input
              className="input"
              value={profile.name}
              placeholder={t('profiles.name')}
              aria-label={t('profiles.name')}
              onChange={(e) => change(index, { name: e.target.value })}
            />
            <button
              type="button"
              className="icon-btn danger"
              aria-label={t('profiles.remove')}
              title={t('profiles.remove')}
              onClick={() => update({ download_profiles: profiles.filter((_, i) => i !== index) })}
            >
              <Trash2 size={14} aria-hidden={true} />
            </button>
          </div>
          <div className="profile-grid">
            <input
              className="input"
              value={profile.url_pattern}
              placeholder={t('profiles.host')}
              aria-label={t('profiles.host')}
              onChange={(e) => change(index, { url_pattern: e.target.value })}
              spellCheck={false}
            />
            <input
              className="input"
              value={profile.save_subdir ?? ''}
              placeholder={t('profiles.subfolder')}
              aria-label={t('profiles.subfolder')}
              onChange={(e) => change(index, { save_subdir: text(e.target.value) })}
              spellCheck={false}
            />
            <input
              className="input"
              value={profile.referrer ?? ''}
              placeholder={t('profiles.referrer')}
              aria-label={t('profiles.referrer')}
              onChange={(e) => change(index, { referrer: text(e.target.value) })}
              spellCheck={false}
            />
            <input
              className="input"
              value={profile.user_agent ?? ''}
              placeholder={t('profiles.user_agent')}
              aria-label={t('profiles.user_agent')}
              onChange={(e) => change(index, { user_agent: text(e.target.value) })}
              spellCheck={false}
            />
          </div>
          <input
            className="input"
            type="password"
            autoComplete="off"
            value={profile.cookies ?? ''}
            placeholder={t('profiles.cookies')}
            aria-label={t('profiles.cookies_label')}
            onChange={(e) => change(index, { cookies: text(e.target.value) })}
          />
        </div>
      ))}
      <button
        type="button"
        className="btn start"
        onClick={() => update({ download_profiles: [...profiles, { ...EMPTY }] })}
      >
        <Plus size={14} aria-hidden={true} /> {t('profiles.add')}
      </button>
    </>
  );
}
