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
            <GeneralTab
              settings={settings}
              setSettings={setSettings}
              handleChange={handleChange}
              handleBrowse={handleBrowse}
              pendingPairs={pendingPairs}
              handleApprovePair={handleApprovePair}
              handleRelinkExtension={handleRelinkExtension}
            />
          )}
          {activeTab === 'network' && (
            <NetworkTab settings={settings} handleChange={handleChange} />
          )}
          {activeTab === 'profiles' && (
            <ProfilesTab settings={settings} setSettings={setSettings} />
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
