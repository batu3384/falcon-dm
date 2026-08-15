import { useTranslation } from 'react-i18next';
import type { SettingsModel } from '../../types';

type Props = {
  settings: SettingsModel;
  handleChange: (field: keyof SettingsModel, value: string | number | null) => void;
};

export function NetworkTab({ settings, handleChange }: Props) {
  const { t } = useTranslation();
  return (
    <>
      <div className="field">
        <label className="field-label" htmlFor="set-concurrent">
          {t('settings.max_concurrent')} ({settings.max_concurrent_downloads})
        </label>
        <input
          id="set-concurrent"
          type="range"
          min="1"
          max="32"
          value={settings.max_concurrent_downloads}
          onChange={(e) => handleChange('max_concurrent_downloads', parseInt(e.target.value))}
          className="field-range"
        />
      </div>
      <div className="field">
        <label className="field-label" htmlFor="set-conn">
          {t('settings.max_connections')}
        </label>
        <input
          id="set-conn"
          type="number"
          min="1"
          max="16"
          value={settings.max_connections_per_server}
          onChange={(e) => handleChange('max_connections_per_server', parseInt(e.target.value))}
          className="field-input"
        />
        <p className="field-hint">{t('settings.max_connections_hint')}</p>
      </div>
      <div className="field">
        <label className="field-label" htmlFor="set-speed">
          {t('settings.speed_limit')}
        </label>
        <input
          id="set-speed"
          type="number"
          min="0"
          max="1048576"
          value={settings.speed_limit_kbps || 0}
          onChange={(e) => handleChange('speed_limit_kbps', parseInt(e.target.value) || 0)}
          className="field-input"
        />
      </div>
      <div className="field">
        <label className="field-label" htmlFor="set-proxy">
          {t('settings.proxy')}
        </label>
        <input
          id="set-proxy"
          type="text"
          placeholder={t('settings.proxy_placeholder')}
          value={settings.proxy || ''}
          onChange={(e) => handleChange('proxy', e.target.value || null)}
          className="field-input"
        />
      </div>
    </>
  );
}
