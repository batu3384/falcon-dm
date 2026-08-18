import { useTranslation } from 'react-i18next';
import type { SettingsModel } from '../../types';

type Props = {
  settings: SettingsModel;
  setSettings: (next: SettingsModel | ((previous: SettingsModel) => SettingsModel)) => void;
  handleChange: (field: keyof SettingsModel, value: string | number | null) => void;
  handleBrowse: () => void;
  pendingPairs: string[];
  handleApprovePair: (extensionId: string) => void;
  handleRelinkExtension: () => void;
  extensionId: string;
  onExtensionIdChange: (value: string) => void;
  onInstallNativeHost: () => void;
  installingNativeHost: boolean;
};

export function GeneralTab({
  settings,
  setSettings,
  handleChange,
  handleBrowse,
  pendingPairs,
  handleApprovePair,
  handleRelinkExtension,
  extensionId,
  onExtensionIdChange,
  onInstallNativeHost,
  installingNativeHost,
}: Props) {
  const { t } = useTranslation();
  return (
    <>
      <div className="field">
        <label className="field-label" htmlFor="set-theme">
          {t('settings.theme')}
        </label>
        <select
          id="set-theme"
          className="field-input field-select"
          value={settings.theme}
          onChange={(e) => handleChange('theme', e.target.value)}
        >
          <option value="system">{t('settings.theme_system')}</option>
          <option value="light">{t('settings.theme_light')}</option>
          <option value="dark">{t('settings.theme_dark')}</option>
        </select>
      </div>
      <div className="field">
        <label className="field-label" htmlFor="set-path">
          {t('settings.download_path')}
        </label>
        <div className="input-action">
          <input
            id="set-path"
            className="field-input"
            type="text"
            value={settings.default_download_path}
            onChange={(e) => handleChange('default_download_path', e.target.value)}
          />
          <button type="button" className="btn-secondary" onClick={handleBrowse}>
            {t('settings.browse')}
          </button>
        </div>
      </div>
      <div className="field">
        <label className="field-label" htmlFor="set-ytdlp">
          {t('settings.ytdlp_path')}
        </label>
        <input
          id="set-ytdlp"
          className="field-input"
          type="text"
          value={settings.ytdlp_path || ''}
          onChange={(e) => handleChange('ytdlp_path', e.target.value)}
          placeholder="/opt/homebrew/bin/yt-dlp"
        />
        <p className="field-hint">{t('settings.ytdlp_path_hint')}</p>
      </div>
      <div className="field">
        <label className="field-label" htmlFor="set-token">
          {t('settings.api_token')}
        </label>
        <input
          id="set-token"
          className="field-input mono"
          type="password"
          autoComplete="off"
          placeholder={t('settings.api_token_keep')}
          value={settings.api_token || ''}
          onChange={(e) => handleChange('api_token', e.target.value)}
        />
        <p className="field-hint">{t('settings.api_token_keep')}</p>
        <button
          type="button"
          className="btn-secondary"
          onClick={() => handleChange('api_token', crypto.randomUUID())}
        >
          {t('settings.api_token_rotate')}
        </button>
      </div>
      <div className="field">
        <label className="field-label">{t('settings.extension')}</label>
        <p className="field-hint">{t('settings.extension_hint')}</p>
        {pendingPairs.length > 0 ? (
          pendingPairs.map((extensionId) => (
            <div className="input-action" style={{ marginBottom: 8 }} key={extensionId}>
              <code className="field-hint" style={{ flex: 1 }}>
                {extensionId}
              </code>
              <button
                type="button"
                className="btn-primary"
                onClick={() => handleApprovePair(extensionId)}
              >
                {t('settings.pair_approve')}
              </button>
            </div>
          ))
        ) : (
          <p className="field-hint">{t('settings.pair_none')}</p>
        )}
        <button type="button" className="btn-secondary" onClick={handleRelinkExtension}>
          {t('settings.relink_extension')}
        </button>
        <div className="field" style={{ marginTop: 12 }}>
          <label className="field-label" htmlFor="set-extension-id">
            {t('onboarding.native_host_id')}
          </label>
          <div className="input-action">
            <input
              id="set-extension-id"
              className="field-input mono"
              type="text"
              value={extensionId}
              onChange={(e) => onExtensionIdChange(e.target.value)}
              placeholder="abcdefghijklmnopqrstuvwxyzabcdef"
              maxLength={32}
            />
            <button
              type="button"
              className="btn-secondary"
              onClick={onInstallNativeHost}
              disabled={installingNativeHost}
            >
              {installingNativeHost
                ? t('onboarding.native_host_installing')
                : t('onboarding.native_host_install')}
            </button>
          </div>
          <p className="field-hint">{t('onboarding.native_host_hint')}</p>
        </div>
      </div>
      <div className="field">
        <label className="field-label" htmlFor="cat-video">
          {t('settings.cat_video')}
        </label>
        <input
          id="cat-video"
          className="field-input"
          type="text"
          value={settings.category_paths?.Video || ''}
          onChange={(e) =>
            setSettings((prev) => ({
              ...prev,
              category_paths: { ...(prev.category_paths || {}), Video: e.target.value },
            }))
          }
          placeholder="~/Movies"
        />
      </div>
      <div className="field">
        <label className="field-label" htmlFor="cat-music">
          {t('settings.cat_music')}
        </label>
        <input
          id="cat-music"
          className="field-input"
          type="text"
          value={settings.category_paths?.Music || ''}
          onChange={(e) =>
            setSettings((prev) => ({
              ...prev,
              category_paths: { ...(prev.category_paths || {}), Music: e.target.value },
            }))
          }
          placeholder="~/Music"
        />
      </div>
      <div className="field">
        <label className="field-label" htmlFor="cat-docs">
          {t('settings.cat_documents')}
        </label>
        <input
          id="cat-docs"
          className="field-input"
          type="text"
          value={settings.category_paths?.Document || ''}
          onChange={(e) =>
            setSettings((prev) => ({
              ...prev,
              category_paths: { ...(prev.category_paths || {}), Document: e.target.value },
            }))
          }
          placeholder="~/Documents"
        />
      </div>
      <div className="field">
        <label className="field-label" htmlFor="cat-arch">
          {t('settings.cat_compressed')}
        </label>
        <input
          id="cat-arch"
          className="field-input"
          type="text"
          value={settings.category_paths?.Archive || ''}
          onChange={(e) =>
            setSettings((prev) => ({
              ...prev,
              category_paths: { ...(prev.category_paths || {}), Archive: e.target.value },
            }))
          }
          placeholder="~/Downloads"
        />
      </div>
      <div className="field">
        <label className="field-label" htmlFor="cat-prog">
          {t('settings.cat_programs')}
        </label>
        <input
          id="cat-prog"
          className="field-input"
          type="text"
          value={settings.category_paths?.Program || ''}
          onChange={(e) =>
            setSettings((prev) => ({
              ...prev,
              category_paths: { ...(prev.category_paths || {}), Program: e.target.value },
            }))
          }
          placeholder="~/Applications"
        />
      </div>
    </>
  );
}
