import { describe, expect, it } from 'vitest';
import { isValidScheduleRange } from '../lib/scheduler';

describe('isValidScheduleRange', () => {
  it('accepts overnight ranges', () => {
    expect(isValidScheduleRange('23:00', '06:00')).toBe(true);
  });

  it('rejects equal and malformed ranges', () => {
    expect(isValidScheduleRange('06:00', '06:00')).toBe(false);
    expect(isValidScheduleRange('6:00', '07:00')).toBe(false);
  });
});
