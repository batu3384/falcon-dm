import { useEffect, useState } from 'react';
import { useTranslation } from 'react-i18next';
import { CheckCircle2, AlertTriangle } from 'lucide-react';
import * as api from '../../api/commands';
import type { ToolStatus } from '../../types';
import { displayPath } from '../../types';
import { Check, FormRow } from './FormRow';
import type { PaneProps } from './types';

function ToolLine({ ok, children }: { ok: boolean; children: React.ReactNode }) {
  return (
    <div className="tool-state">
      {ok ? (
        <CheckCircle2 size={14} color="var(--green)" aria-hidden={true} />
      ) : (
        <AlertTriangle size={14} color="var(--orange)" aria-hidden={true} />
      )}
      {children}
    </div>
  );
}

export function NetworkPane({ settings, update }: PaneProps) {
  const { t } = useTranslation();
  const [tools, setTools] = useState<ToolStatus | null>(null);
  const [toolsError, setToolsError] = useState(false);

  useEffect(() => {
    let cancelled = false;
    api
      .getToolStatus()
      .then((status) => !cancelled && setTools(status))
      .catch(() => !cancelled && setToolsError(true));
    return () => {
      cancelled = true;
    };
  }, []);

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
      <div className="form-sep" />
      <FormRow label="yt-dlp" plain>
        {tools ? (
          tools.ytdlp_path ? (
            <ToolLine ok>
              <span>{tools.ytdlp_version ? `yt-dlp ${tools.ytdlp_version}` : 'yt-dlp'}</span>
              <span className="sub" title={tools.ytdlp_path}>
                {displayPath(tools.ytdlp_path)}
              </span>
            </ToolLine>
          ) : (
            <ToolLine ok={false}>
              <span>
                {tools.ytdlp_error?.startsWith('ERR_YTDLP_PATH_INVALID')
                  ? t('settings.ytdlp_bad_path')
                  : t('settings.ytdlp_missing')}
              </span>
            </ToolLine>
          )
        ) : (
          <span className="hint">
            {toolsError ? t('settings.tools_unknown') : t('common.checking')}
          </span>
        )}
        {tools && !tools.ytdlp_path && (
          <p className="hint">
            {t('settings.ytdlp_install')}{' '}
            <code className="code-chip selectable">brew install yt-dlp</code>
          </p>
        )}
      </FormRow>
      <FormRow label={t('settings.ytdlp_path')} htmlFor="set-ytdlp">
        <input
          id="set-ytdlp"
          className="input"
          value={settings.ytdlp_path ?? ''}
          placeholder={t('settings.ytdlp_path_placeholder')}
          onChange={(e) => update({ ytdlp_path: e.target.value })}
          spellCheck={false}
        />
      </FormRow>
      <FormRow label={t('settings.browser_cookies')} plain>
        <Check
          id="set-ytdlp-cookies"
          checked={!!settings.ytdlp_use_browser_cookies}
          onChange={(on) => update({ ytdlp_use_browser_cookies: on })}
          label={t('settings.browser_cookies_label')}
          hint={t('settings.browser_cookies_hint')}
        />
      </FormRow>
      <FormRow label="ffmpeg" plain>
        {tools ? (
          tools.ffmpeg_path ? (
            <ToolLine ok>
              <span className="sub" title={tools.ffmpeg_path}>
                {displayPath(tools.ffmpeg_path)}
              </span>
            </ToolLine>
          ) : (
            <ToolLine ok={false}>
              <span>{t('settings.ffmpeg_missing')}</span>
            </ToolLine>
          )
        ) : (
          <span className="hint">
            {toolsError ? t('settings.tools_unknown') : t('common.checking')}
          </span>
        )}
      </FormRow>
    </>
  );
}
