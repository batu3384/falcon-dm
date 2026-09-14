import { useState, useEffect, useRef, useCallback } from 'react';
import { open } from '@tauri-apps/plugin-dialog';
import { useTranslation } from 'react-i18next';
import { X } from 'lucide-react';
import type { SettingsModel } from '../types';
import { applyTheme } from '../lib/theme';
import { GeneralTab } from './settings/GeneralTab';
import { NetworkTab } from './settings/NetworkTab';
import { ProfilesTab } from './settings/ProfilesTab';
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
    ytdlp_use_browser_cookies: false,
    download_profiles: [],
  });
  const [saveError, setSaveError] = useState('');
  const [loadError, setLoadError] = useState('');
  const [pendingPairs, setPendingPairs] = useState<string[]>([]);
  const [saving, setSaving] = useState(false);
  const [hydrated, setHydrated] = useState(false);
  const [loadNonce, setLoadNonce] = useState(0);
  const [confirmClose, setConfirmClose] = useState(false);
  const [extensionId, setExtensionId] = useState('');
  const [installingNativeHost, setInstallingNativeHost] = useState(false);
  const dirtyRef = useRef(false);
  const SETTINGS_TABS = ['general', 'network', 'profiles'] as const;

  const onTabListKeyDown = (e: React.KeyboardEvent) => {
    if (
      e.key !== 'ArrowRight' &&
      e.key !== 'ArrowLeft' &&
      e.key !== 'ArrowDown' &&
      e.key !== 'ArrowUp' &&
      e.key !== 'Home' &&
      e.key !== 'End'
    ) {
      return;
    }
    e.preventDefault();
    const i = SETTINGS_TABS.indexOf(activeTab);
    let next = i;
    if (e.key === 'ArrowRight' || e.key === 'ArrowDown') next = (i + 1) % SETTINGS_TABS.length;
    else if (e.key === 'ArrowLeft' || e.key === 'ArrowUp')
      next = (i - 1 + SETTINGS_TABS.length) % SETTINGS_TABS.length;
    else if (e.key === 'Home') next = 0;
    else next = SETTINGS_TABS.length - 1;
    const id = SETTINGS_TABS[next];
    setActiveTab(id);
    requestAnimationFrame(() => document.getElementById(`settings-tab-${id}`)?.focus());
  };

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
        setLoadError('');
        // First snapshot always wins so Save cannot persist the empty initial state.
        setSettingsState(loaded);
        const suggested = loaded.allowed_extension_ids?.[0];
        if (suggested) setExtensionId(suggested);
        dirtyRef.current = false;
        setHydrated(true);
      })
      .catch((e) => {
        setLoadError(api.extractTauriError(e) || t('settings.load_failed'));
      });
    api
      .getPendingPairs()
      .then(setPendingPairs)
      .catch(() => {
        showToast('error', t('settings.pair_load_failed'));
      });
  }, [t, loadNonce, showToast]);

  useEffect(() => {
    const unlisten = onPairRequest((extensionId) => {
      setPendingPairs((current) =>
        current.includes(extensionId) ? current : [...current, extensionId],
      );
      setExtensionId((current) => current || extensionId);
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
      setExtensionId(extensionId);
      showToast('success', t('settings.pair_approved'));
    } catch (e) {
      console.error(e);
      showToast('error', t('settings.save_failed'));
    }
  };

  const handleInstallNativeHost = async () => {
    const chromeId = extensionId.trim();
    if (!/^[a-p]{32}$/.test(chromeId)) {
      showToast('error', t('onboarding.native_host_invalid_id'));
      return;
    }
    setInstallingNativeHost(true);
    try {
      await api.installNativeHostManifests(chromeId);
      showToast('success', t('onboarding.native_host_installed'));
    } catch (e) {
      console.error(e);
      showToast('error', t('onboarding.native_host_failed'));
    } finally {
      setInstallingNativeHost(false);
    }
  };

  const handleSave = async () => {
    if (saving || loadError || !hydrated) return;
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
            <X size={18} aria-hidden={true} />
          </button>
        </div>

        <div className="modal-body" {...(!hydrated ? { inert: true } : {})}>
          <div
            className="tabs"
            role="tablist"
            aria-label={t('settings.title')}
            onKeyDown={onTabListKeyDown}
          >
            {(
              [
                ['general', t('settings.general')],
                ['network', t('settings.network')],
                ['profiles', t('settings.profiles')],
              ] as const
            ).map(([id, label]) => (
              <button
                key={id}
                type="button"
                id={`settings-tab-${id}`}
                role="tab"
                aria-selected={activeTab === id}
                aria-controls={`settings-panel-${id}`}
                tabIndex={activeTab === id ? 0 : -1}
                className={`tab ${activeTab === id ? 'active' : ''}`}
                onClick={() => setActiveTab(id)}
              >
                {label}
              </button>
            ))}
          </div>

          <div
            id="settings-panel-general"
            role="tabpanel"
            aria-labelledby="settings-tab-general"
            hidden={activeTab !== 'general'}
          >
            <GeneralTab
              settings={settings}
              setSettings={setSettings}
              handleChange={handleChange}
              handleBrowse={handleBrowse}
              pendingPairs={pendingPairs}
              handleApprovePair={handleApprovePair}
              handleRelinkExtension={handleRelinkExtension}
              extensionId={extensionId}
              onExtensionIdChange={setExtensionId}
              onInstallNativeHost={handleInstallNativeHost}
              installingNativeHost={installingNativeHost}
            />
          </div>
          <div
            id="settings-panel-network"
            role="tabpanel"
            aria-labelledby="settings-tab-network"
            hidden={activeTab !== 'network'}
          >
            <NetworkTab settings={settings} handleChange={handleChange} />
          </div>
          <div
            id="settings-panel-profiles"
            role="tabpanel"
            aria-labelledby="settings-tab-profiles"
            aria-describedby="settings-profiles-hint"
            hidden={activeTab !== 'profiles'}
          >
            <ProfilesTab settings={settings} setSettings={setSettings} />
          </div>
          {saveError && (
            <p className="field-error" role="alert">
              {saveError}
            </p>
          )}
        </div>

        <div className="modal-foot">
          {loadError && (
            <p className="field-error" role="alert">
              {loadError}
            </p>
          )}
          <button type="button" className="btn-secondary" data-modal-cancel onClick={requestClose}>
            {t('settings.cancel')}
          </button>
          {loadError ? (
            <button
              type="button"
              className="btn-primary"
              onClick={() => {
                setLoadError('');
                setLoadNonce((n) => n + 1);
              }}
            >
              {t('settings.retry')}
            </button>
          ) : (
            <button
              type="button"
              className="btn-primary"
              onClick={handleSave}
              disabled={saving || !hydrated}
            >
              {saving ? t('settings.saving') : t('settings.save')}
            </button>
          )}
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
