import { describe, expect, it } from 'vitest';
import { isValidSchedule, isValidScheduleRange } from '../lib/scheduler';

describe('isValidScheduleRange', () => {
  it('accepts overnight ranges', () => {
    expect(isValidScheduleRange('23:00', '06:00')).toBe(true);
  });

  it('rejects equal and malformed ranges', () => {
    expect(isValidScheduleRange('06:00', '06:00')).toBe(false);
    expect(isValidScheduleRange('6:00', '07:00')).toBe(false);
    expect(isValidScheduleRange('24:00', '07:00')).toBe(false);
  });
});

describe('isValidSchedule', () => {
  it('allows inactive empty times', () => {
    expect(isValidSchedule({ start_time: null, stop_time: null, active: false })).toBe(true);
  });

  it('requires both ends when active or one side is set', () => {
    expect(isValidSchedule({ start_time: '08:00', stop_time: null, active: false })).toBe(false);
    expect(isValidSchedule({ start_time: null, stop_time: null, active: true })).toBe(false);
  });
});
