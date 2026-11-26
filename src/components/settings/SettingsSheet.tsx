import { useCallback, useEffect, useRef, useState } from 'react';
import { useTranslation } from 'react-i18next';
import { Download, Globe, Puzzle, Settings2, Users } from 'lucide-react';
import { Sheet } from '../Sheet';
import { ConfirmDialog } from '../ConfirmDialog';
import * as api from '../../api/commands';
import type { SettingsModel } from '../../types';
import { applyTheme } from '../../lib/theme';
import { isValidSchedule } from '../../lib/scheduler';
import { rememberSpeedLimit } from '../../lib/speedLimit';
import { applyLanguagePreference, languagePreference } from '../../i18n';
import { useAppStore } from '../../store/app';
import { useToastStore } from '../../store/toast';
import type { SettingsTab } from '../../store/ui';
import { GeneralPane } from './GeneralPane';
import { DownloadsPane } from './DownloadsPane';
import { NetworkPane } from './NetworkPane';
import { ExtensionPane } from './ExtensionPane';
import { ProfilesPane } from './ProfilesPane';
import type { Prefs, ScheduleDraft } from './types';

const TABS: { id: SettingsTab; icon: typeof Download }[] = [
  { id: 'general', icon: Settings2 },
  { id: 'downloads', icon: Download },
  { id: 'network', icon: Globe },
  { id: 'extension', icon: Puzzle },
  { id: 'profiles', icon: Users },
];

export const CLIPBOARD_KEY = 'falcon_clipboard_monitor';

interface SettingsSheetProps {
  initialTab: SettingsTab;
  onClose: () => void;
  onSaved?: () => void;
}

export function SettingsSheet({ initialTab, onClose, onSaved }: SettingsSheetProps) {
  const { t } = useTranslation();
  const showToast = useToastStore((s) => s.showToast);
  const refreshExtension = useAppStore((s) => s.refreshExtension);
  const [tab, setTab] = useState<SettingsTab>(initialTab);
  const [settings, setSettings] = useState<SettingsModel | null>(null);
  const [schedule, setScheduleState] = useState<ScheduleDraft>({
    active: false,
    start: '',
    stop: '',
  });
  const [prefs, setPrefsState] = useState<Prefs>(() => ({
    language: languagePreference(),
    clipboardMonitor: localStorage.getItem(CLIPBOARD_KEY) === '1',
  }));
  const [loadError, setLoadError] = useState('');
  const [loadNonce, setLoadNonce] = useState(0);
  const [saving, setSaving] = useState(false);
  const [scheduleError, setScheduleError] = useState('');
  const [confirmClose, setConfirmClose] = useState(false);
  const dirty = useRef(false);

  useEffect(() => {
    let cancelled = false;
    Promise.all([api.getSettings(), api.getSchedule()])
      .then(([loaded, sched]) => {
        if (cancelled) return;
        setSettings(loaded);
        setScheduleState({
          active: sched.active,
          start: sched.start_time ?? '',
          stop: sched.stop_time ?? '',
        });
        setLoadError('');
        dirty.current = false;
      })
      .catch((e) => !cancelled && setLoadError(api.extractTauriError(e)));
    void refreshExtension();
    return () => {
      cancelled = true;
    };
  }, [loadNonce, refreshExtension]);

  const update = useCallback((patch: Partial<SettingsModel>) => {
    dirty.current = true;
    setSettings((current) => (current ? { ...current, ...patch } : current));
  }, []);
  const setSchedule = (patch: Partial<ScheduleDraft>) => {
    dirty.current = true;
    setScheduleError('');
    setScheduleState((current) => ({ ...current, ...patch }));
  };
  const setPrefs = (patch: Partial<Prefs>) => {
    dirty.current = true;
    setPrefsState((current) => ({ ...current, ...patch }));
  };

  const requestClose = useCallback(() => {
    if (saving) return;
    if (dirty.current) setConfirmClose(true);
    else onClose();
  }, [onClose, saving]);

  const save = async () => {
    if (!settings || saving) return;
    const draft = {
      start_time: schedule.start || null,
      stop_time: schedule.stop || null,
      active: schedule.active,
    };
    if (!isValidSchedule(draft)) {
      setTab('downloads');
      setScheduleError(t('settings.schedule_invalid'));
      return;
    }
    setSaving(true);
    try {
      await api.saveSettings(settings);
      await api.setSchedule({
        startTime: draft.start_time,
        stopTime: draft.stop_time,
        active: draft.active,
      });
      rememberSpeedLimit(settings.speed_limit_kbps ?? 0);
      applyTheme(settings.theme);
      localStorage.setItem(CLIPBOARD_KEY, prefs.clipboardMonitor ? '1' : '0');
      applyLanguagePreference(prefs.language);
      dirty.current = false;
      onSaved?.();
      onClose();
    } catch (e) {
      showToast('error', `${t('settings.save_failed')}: ${api.extractTauriError(e)}`);
    } finally {
      setSaving(false);
    }
  };

  const onTabKey = (e: React.KeyboardEvent) => {
    const index = TABS.findIndex((item) => item.id === tab);
    const delta = e.key === 'ArrowRight' ? 1 : e.key === 'ArrowLeft' ? -1 : 0;
    if (!delta) return;
    e.preventDefault();
    const next = TABS[(index + delta + TABS.length) % TABS.length].id;
    setTab(next);
    document.getElementById(`settings-tab-${next}`)?.focus();
  };

  return (
    <>
      <Sheet
        size="wide"
        label={t('settings.title')}
        onClose={requestClose}
        footer={
          <>
            {loadError && (
              <span className="field-error grow" role="alert">
                {t('settings.load_failed')}
              </span>
            )}
            <button type="button" className="btn" data-modal-cancel onClick={requestClose}>
              {t('common.cancel')}
            </button>
            {loadError ? (
              <button
                type="button"
                className="btn primary"
                onClick={() => setLoadNonce((n) => n + 1)}
              >
                {t('common.retry')}
              </button>
            ) : (
              <button
                type="button"
                className="btn primary"
                onClick={() => void save()}
                disabled={!settings || saving}
              >
                {saving ? t('common.saving') : t('common.save')}
              </button>
            )}
          </>
        }
      >
        <div className="tabs" role="tablist" aria-label={t('settings.title')} onKeyDown={onTabKey}>
          {TABS.map(({ id, icon: Icon }) => (
            <button
              key={id}
              id={`settings-tab-${id}`}
              type="button"
              role="tab"
              className="tab"
              aria-selected={tab === id}
              aria-controls="settings-panel"
              tabIndex={tab === id ? 0 : -1}
              data-modal-initial-focus={tab === id ? true : undefined}
              onClick={() => setTab(id)}
            >
              <Icon size={20} strokeWidth={1.6} aria-hidden={true} />
              {t(`settings.tab_${id}`)}
            </button>
          ))}
        </div>
        <div
          id="settings-panel"
          className="sheet-body settings-body"
          role="tabpanel"
          aria-labelledby={`settings-tab-${tab}`}
        >
          {tab === 'extension' ? (
            <ExtensionPane />
          ) : !settings ? (
            <p className="hint" role="status">
              {loadError || t('common.loading')}
            </p>
          ) : tab === 'general' ? (
            <GeneralPane settings={settings} update={update} prefs={prefs} setPrefs={setPrefs} />
          ) : tab === 'downloads' ? (
            <DownloadsPane
              settings={settings}
              update={update}
              schedule={schedule}
              setSchedule={setSchedule}
              scheduleError={scheduleError}
            />
          ) : tab === 'network' ? (
            <NetworkPane settings={settings} update={update} />
          ) : (
            <ProfilesPane settings={settings} update={update} />
          )}
        </div>
      </Sheet>
      {confirmClose && (
        <ConfirmDialog
          message={t('settings.discard_title')}
          detail={t('settings.discard_detail')}
          confirmLabel={t('common.discard')}
          cancelLabel={t('common.keep_editing')}
          onConfirm={() => {
            setConfirmClose(false);
            onClose();
          }}
          onCancel={() => setConfirmClose(false)}
        />
      )}
    </>
  );
}
