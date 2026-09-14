import { describe, expect, it } from 'vitest';
import { speedLimitFromSettings } from './speedLimit';

describe('speedLimitFromSettings', () => {
  it('turns the Gauge on and remembers a positive limit', () => {
    expect(speedLimitFromSettings(2048, 1024)).toEqual({ limited: true, saved: 2048 });
  });

  it('keeps the last non-zero when Settings clears the cap', () => {
    expect(speedLimitFromSettings(0, 1024)).toEqual({ limited: false, saved: 1024 });
    expect(speedLimitFromSettings(undefined, 512)).toEqual({ limited: false, saved: 512 });
    expect(speedLimitFromSettings(Number.NaN, 512)).toEqual({ limited: false, saved: 512 });
  });
});
