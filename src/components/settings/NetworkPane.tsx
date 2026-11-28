import { useTranslation } from 'react-i18next';
import { FormRow } from './FormRow';
import type { PaneProps } from './types';

export function NetworkPane({ settings, update }: PaneProps) {
  const { t } = useTranslation();

  return (
    <>
      <FormRow label={t('settings.proxy')} htmlFor="set-proxy">
        <input
          id="set-proxy"
          className="input"
          value={settings.proxy ?? ''}
          placeholder="http://127.0.0.1:8080"
          onChange={(e) => update({ proxy: e.target.value || null })}
          spellCheck={false}
          aria-describedby="set-proxy-hint"
        />
        <p id="set-proxy-hint" className="hint">
          {t('settings.proxy_hint')}
        </p>
      </FormRow>
    </>
  );
}
