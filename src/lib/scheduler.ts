/** Keep rules aligned with `validate_schedule` in queue.rs. */

export type ScheduleCheck = {
  start_time?: string | null;
  stop_time?: string | null;
  active: boolean;
};

function parseScheduleTime(value: string): 'missing' | 'invalid' | string {
  // Rust: non-HH:MM digit form is treated as absent; NaiveTime rejects 24:00/23:60.
  if (
    value.length !== 5 ||
    value.charCodeAt(2) !== 58 ||
    ![...value.slice(0, 2), ...value.slice(3)].every((ch) => ch >= '0' && ch <= '9')
  ) {
    return 'missing';
  }
  const hour = Number(value.slice(0, 2));
  const minute = Number(value.slice(3));
  if (hour > 23 || minute > 59) return 'invalid';
  return value;
}

export function isValidSchedule(opts: ScheduleCheck): boolean {
  const parse = (raw: string | null | undefined) => {
    if (raw == null || raw === '') return null;
    const parsed = parseScheduleTime(raw);
    if (parsed === 'invalid') return false;
    if (parsed === 'missing') return null;
    return parsed;
  };
  const start = parse(opts.start_time);
  const stop = parse(opts.stop_time);
  if (start === false || stop === false) return false;
  if (opts.active && (start === null || stop === null)) return false;
  if ((start === null) !== (stop === null)) return false;
  if (start !== null && start === stop) return false;
  return true;
}

export function isValidScheduleRange(start: string, stop: string): boolean {
  return isValidSchedule({ start_time: start, stop_time: stop, active: true });
}
