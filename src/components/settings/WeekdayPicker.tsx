import { useTranslation } from 'react-i18next';
import type { WeekdayMask } from '../../lib/scheduler';

const WEEKDAYS = [
  'monday',
  'tuesday',
  'wednesday',
  'thursday',
  'friday',
  'saturday',
  'sunday',
] as const;

export function WeekdayPicker({
  value,
  disabled = false,
  onChange,
}: {
  value: WeekdayMask;
  disabled?: boolean;
  onChange: (days: WeekdayMask) => void;
}) {
  const { t } = useTranslation();
  return (
    <div className="weekday-picker" role="group" aria-label={t('settings.schedule_days')}>
      {WEEKDAYS.map((day, index) => (
        <button
          key={day}
          type="button"
          disabled={disabled}
          aria-pressed={value[index]}
          aria-label={t(`settings.weekday_${day}`)}
          onClick={() => {
            const next = [...value] as WeekdayMask;
            next[index] = !next[index];
            onChange(next);
          }}
        >
          {t(`settings.weekday_${day}_short`)}
        </button>
      ))}
    </div>
  );
}
