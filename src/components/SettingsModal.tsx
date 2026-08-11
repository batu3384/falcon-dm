import { useState, useEffect, useRef, useCallback } from 'react';
import { open } from '@tauri-apps/plugin-dialog';
import { useTranslation } from 'react-i18next';
import { X, Trash2, Plus } from 'lucide-react';
import type { SettingsModel } from '../types';
import { applyTheme } from '../types';
import { useModalA11y } from '../hooks/useModalA11y';
import { ConfirmDialog } from './ConfirmDialog';
import { useToastStore } from '../store/toast';
import { onPairRequest } from '../api/events';
import * as api from '../api/commands';

interface SettingsModalProps {
  onClose: () => void;
}

export const SettingsModal = ({ onClose }: SettingsModalProps) => {
  const { t } = useTranslation();
  const showToast = useToastStore((s) => s.showToast);
  const panelRef = useRef<HTMLDivElement>(null);
  const [activeTab, setActiveTab] = useState<'general' | 'network' | 'profiles'>('general');
  const [settings, setSettingsState] = useState<SettingsModel>({
    theme: 'system',
    default_download_path: '~/Downloads',
    max_concurrent_downloads: 3,
    max_connections_per_server: 16,
    proxy: null,
    api_token: '',
    speed_limit_kbps: 0,
    category_paths: {},
    allowed_extension_ids: [],
    ytdlp_path: '',
    download_profiles: [],
  });
  const [saveError, setSaveError] = useState('');
  const [pendingPairs, setPendingPairs] = useState<string[]>([]);
  const [saving, setSaving] = useState(false);
  const [confirmClose, setConfirmClose] = useState(false);
  const dirtyRef = useRef(false);

  const updateSettings = (next: SettingsModel | ((previous: SettingsModel) => SettingsModel)) => {
    dirtyRef.current = true;
    setSettingsState(next);
  };
  const setSettings = updateSettings;

  const requestClose = useCallback(() => {
    if (dirtyRef.current && !saving) setConfirmClose(true);
    else onClose();
  }, [onClose, saving]);

  useModalA11y(panelRef, requestClose);

  useEffect(() => {
    api
      .getSettings()
      .then((loaded) => {
        if (!dirtyRef.current) setSettingsState(loaded);
      })
      .catch((e) => console.error('Failed to load settings:', e));
    api
      .getPendingPairs()
      .then(setPendingPairs)
      .catch(() => {});
  }, []);

  useEffect(() => {
    const unlisten = onPairRequest((extensionId) => {
      setPendingPairs((current) =>
        current.includes(extensionId) ? current : [...current, extensionId],
      );
    });
    return () => {
      unlisten.then((f) => f());
    };
  }, []);

  const handleChange = (field: keyof SettingsModel, value: string | number | null) => {
    let nextValue = value;
    if (field === 'max_concurrent_downloads') {
      const numeric = typeof value === 'number' && Number.isFinite(value) ? value : 1;
      nextValue = Math.min(32, Math.max(1, numeric));
    } else if (field === 'max_connections_per_server') {
      const numeric = typeof value === 'number' && Number.isFinite(value) ? value : 1;
      nextValue = Math.min(16, Math.max(1, numeric));
    } else if (field === 'speed_limit_kbps') {
      const numeric = typeof value === 'number' && Number.isFinite(value) ? value : 0;
      nextValue = Math.min(1_048_576, Math.max(0, numeric));
    }
    updateSettings((prev) => ({ ...prev, [field]: nextValue }));
  };

  const handleBrowse = async () => {
    try {
      const selected = await open({ directory: true, multiple: false });
      if (selected && typeof selected === 'string') handleChange('default_download_path', selected);
    } catch (e) {
      console.error('Browse error:', e);
    }
  };

  const handleRelinkExtension = async () => {
    try {
      await api.resetExtensionPin();
      setPendingPairs([]);
      showToast('success', t('settings.extension_reset'));
    } catch (e) {
      console.error(e);
      showToast('error', t('settings.save_failed'));
    }
  };

  const handleApprovePair = async (extensionId: string) => {
    try {
      await api.approveExtensionPair(extensionId);
      setPendingPairs(await api.getPendingPairs());
      const s = await api.getSettings();
      if (!dirtyRef.current) setSettingsState(s);
      showToast('success', t('settings.pair_approved'));
    } catch (e) {
      console.error(e);
      showToast('error', t('settings.save_failed'));
    }
  };

  const handleSave = async () => {
    if (saving) return;
    setSaveError('');
    setSaving(true);
    try {
      await api.saveSettings(settings);
      applyTheme(settings.theme);
      dirtyRef.current = false;
      showToast('success', t('settings.saved'));
      onClose();
    } catch (e) {
      console.error('Failed to save settings:', e);
      setSaveError(t('settings.save_failed'));
      showToast('error', t('settings.save_failed'));
    } finally {
      setSaving(false);
    }
  };

  return (
    <div className="modal-overlay" onClick={requestClose} role="presentation">
      <div
        ref={panelRef}
        className="modal-panel modal-lg"
        onClick={(e) => e.stopPropagation()}
        role="dialog"
        aria-modal="true"
        aria-labelledby="settings-title"
      >
        <div className="modal-head">
          <h2 id="settings-title" className="modal-title">
            {t('settings.title')}
          </h2>
          <button
            type="button"
            onClick={requestClose}
            className="icon-btn"
            data-modal-cancel
            aria-label={t('settings.cancel')}
          >
            <X size={18} />
          </button>
        </div>

        <div className="modal-body">
          <div className="tabs" role="tablist">
            <button
              type="button"
              role="tab"
              aria-selected={activeTab === 'general'}
              className={`tab ${activeTab === 'general' ? 'active' : ''}`}
              onClick={() => setActiveTab('general')}
            >
              {t('settings.general')}
            </button>
            <button
              type="button"
              role="tab"
              aria-selected={activeTab === 'network'}
              className={`tab ${activeTab === 'network' ? 'active' : ''}`}
              onClick={() => setActiveTab('network')}
            >
              {t('settings.network')}
            </button>
            <button
              type="button"
              role="tab"
              aria-selected={activeTab === 'profiles'}
              className={`tab ${activeTab === 'profiles' ? 'active' : ''}`}
              onClick={() => setActiveTab('profiles')}
            >
              {t('settings.profiles')}
            </button>
          </div>

          {activeTab === 'general' && (
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
                  value={settings.api_token || ''}
                  onChange={(e) => handleChange('api_token', e.target.value)}
                />
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
          )}

          {activeTab === 'network' && (
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
                  onChange={(e) =>
                    handleChange('max_concurrent_downloads', parseInt(e.target.value))
                  }
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
                  onChange={(e) =>
                    handleChange('max_connections_per_server', parseInt(e.target.value))
                  }
                  className="field-input"
                />
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
          )}
          {activeTab === 'profiles' && (
            <>
              <p className="field-hint">{t('settings.profiles_hint')}</p>
              {(settings.download_profiles || []).map((p, i) => (
                <div className="profile-card" key={i}>
                  <div className="profile-card-head">
                    <span className="profile-card-title">
                      {p.name || t('settings.profile_untitled')}
                    </span>
                    <button
                      type="button"
                      className="icon-btn danger"
                      aria-label={t('settings.profile_remove')}
                      onClick={() =>
                        setSettings((prev) => ({
                          ...prev,
                          download_profiles: (prev.download_profiles || []).filter(
                            (_, idx) => idx !== i,
                          ),
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
                      placeholder={t('settings.profile_cookies')}
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
          )}
          {saveError && <p className="field-error">{saveError}</p>}
        </div>

        <div className="modal-foot">
          <button type="button" className="btn-secondary" data-modal-cancel onClick={requestClose}>
            {t('settings.cancel')}
          </button>
          <button type="button" className="btn-primary" onClick={handleSave} disabled={saving}>
            {saving ? t('settings.saving') : t('settings.save')}
          </button>
        </div>
      </div>
      {confirmClose && (
        <ConfirmDialog
          message={t('settings.confirm_close')}
          confirmLabel={t('settings.discard')}
          cancelLabel={t('settings.continue_editing')}
          onConfirm={() => {
            dirtyRef.current = false;
            setConfirmClose(false);
            onClose();
          }}
          onCancel={() => setConfirmClose(false)}
        />
      )}
    </div>
  );
};
