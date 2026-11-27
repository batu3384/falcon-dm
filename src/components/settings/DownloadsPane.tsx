import { useTranslation } from 'react-i18next';
import { open as openDialog } from '@tauri-apps/plugin-dialog';
import { useToastStore } from '../../store/toast';
import * as api from '../../api/commands';
import { Check, FormRow } from './FormRow';
import { NumberField } from './NumberField';
import { lastSpeedLimit, rememberSpeedLimit } from '../../lib/speedLimit';
import type { PaneProps, ScheduleDraft } from './types';

const CATEGORY_FOLDERS = [
  { key: 'Video', label: 'settings.folder_video' },
  { key: 'Music', label: 'settings.folder_music' },
  { key: 'Document', label: 'settings.folder_document' },
  { key: 'Archive', label: 'settings.folder_archive' },
  { key: 'Program', label: 'settings.folder_program' },
] as const;

export function DownloadsPane({
  settings,
  update,
  schedule,
  setSchedule,
  scheduleError,
}: PaneProps & {
  schedule: ScheduleDraft;
  setSchedule: (patch: Partial<ScheduleDraft>) => void;
  scheduleError: string;
}) {
  const { t } = useTranslation();
  const showToast = useToastStore((s) => s.showToast);
  const limit = settings.speed_limit_kbps ?? 0;
  const folders = settings.category_paths ?? {};

  const pickFolder = async (current: string, apply: (path: string) => void) => {
    try {
      const picked = await openDialog({ directory: true, multiple: false, defaultPath: current });
      if (typeof picked === 'string') apply(picked);
    } catch (e) {
      showToast('error', api.extractTauriError(e));
    }
  };

  return (
    <>
      <FormRow label={t('settings.save_folder')} htmlFor="set-folder">
        <div className="input-row">
          <input
            id="set-folder"
            className="input"
            value={settings.default_download_path}
            onChange={(e) => update({ default_download_path: e.target.value })}
            spellCheck={false}
          />
          <button
            type="button"
            className="btn"
            onClick={() =>
              void pickFolder(settings.default_download_path, (path) =>
                update({ default_download_path: path }),
              )
            }
          >
            {t('common.choose')}
          </button>
        </div>
        <p className="hint">{t('settings.save_folder_hint')}</p>
      </FormRow>
      <FormRow label={t('settings.kind_folders')} plain>
        <p className="hint flush">{t('settings.kind_folders_hint')}</p>
      </FormRow>
      {CATEGORY_FOLDERS.map(({ key, label }) => (
        <FormRow key={key} label={t(label)} htmlFor={`set-folder-${key}`}>
          <input
            id={`set-folder-${key}`}
            className="input"
            value={folders[key] ?? ''}
            placeholder={t('settings.same_as_default')}
            onChange={(e) => update({ category_paths: { ...folders, [key]: e.target.value } })}
            spellCheck={false}
          />
        </FormRow>
      ))}
      <div className="form-sep" />
      <FormRow label={t('settings.concurrent')} htmlFor="set-concurrent">
        <div className="stepper">
          <NumberField
            id="set-concurrent"
            min={1}
            max={32}
            value={settings.max_concurrent_downloads}
            onChange={(max_concurrent_downloads) => update({ max_concurrent_downloads })}
          />
          <span className="unit">{t('settings.concurrent_unit')}</span>
        </div>
      </FormRow>
      <FormRow label={t('settings.connections')} htmlFor="set-connections">
        <div className="stepper">
          <NumberField
            id="set-connections"
            min={1}
            max={16}
            value={settings.max_connections_per_server}
            onChange={(max_connections_per_server) => update({ max_connections_per_server })}
            describedBy="set-connections-hint"
          />
          <span className="unit">{t('settings.connections_unit')}</span>
        </div>
        <p id="set-connections-hint" className="hint">
          {t('settings.connections_hint')}
        </p>
      </FormRow>
      <FormRow label={t('settings.speed_limit')} plain>
        <Check
          id="set-limit-on"
          checked={limit > 0}
          onChange={(on) => {
            // Mirror the toolbar toggle: the value turned off is the value restored.
            if (!on) rememberSpeedLimit(limit);
            update({ speed_limit_kbps: on ? lastSpeedLimit() : 0 });
          }}
          label={t('settings.speed_limit_label')}
        />
        {limit > 0 && (
          <div className="stepper">
            <NumberField
              id="set-limit"
              min={1}
              max={1_048_576}
              value={limit}
              label={t('settings.speed_limit')}
              onChange={(speed_limit_kbps) => update({ speed_limit_kbps })}
            />
            <span className="unit">KB/s</span>
          </div>
        )}
      </FormRow>
      <div className="form-sep" />
      <FormRow label={t('settings.schedule')} plain>
        <Check
          id="set-schedule"
          checked={schedule.active}
          onChange={(active) => setSchedule({ active })}
          label={t('settings.schedule_label')}
          hint={t('settings.schedule_hint')}
        />
        <div className="stepper">
          <input
            className="input narrow"
            type="time"
            value={schedule.start}
            disabled={!schedule.active}
            aria-label={t('settings.schedule_start')}
            onChange={(e) => setSchedule({ start: e.target.value })}
            aria-invalid={scheduleError ? true : undefined}
          />
          <span className="unit">–</span>
          <input
            className="input narrow"
            type="time"
            value={schedule.stop}
            disabled={!schedule.active}
            aria-label={t('settings.schedule_stop')}
            onChange={(e) => setSchedule({ stop: e.target.value })}
            aria-invalid={scheduleError ? true : undefined}
          />
        </div>
        {scheduleError && (
          <p className="field-error" role="alert">
            {scheduleError}
          </p>
        )}
      </FormRow>
    </>
  );
}
