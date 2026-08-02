import { describe, it, expect } from 'vitest';
import en from './locales/en.json';
import tr from './locales/tr.json';

// ponytail: en/tr must stay in lockstep. A missing key in one locale would
// silently render the key name itself in the UI. This test flattens both trees
// and asserts identical key sets.

function flatKeys(obj: unknown, prefix = ''): string[] {
  if (obj === null || typeof obj !== 'object') return [];
  if (Array.isArray(obj)) return obj.flatMap((_, i) => `${prefix}[${i}]`);
  return Object.entries(obj as Record<string, unknown>).flatMap(([k, v]) => {
    const key = prefix ? `${prefix}.${k}` : k;
    if (v !== null && typeof v === 'object') return flatKeys(v, key);
    return [key];
  });
}

describe('i18n en/tr parity', () => {
  const enKeys = new Set(flatKeys(en));
  const trKeys = new Set(flatKeys(tr));

  it('tr has every key en has', () => {
    const missing = [...enKeys].filter((k) => !trKeys.has(k));
    expect(missing, `tr.json missing keys: ${missing.join(', ')}`).toEqual([]);
  });

  it('en has every key tr has', () => {
    const missing = [...trKeys].filter((k) => !enKeys.has(k));
    expect(missing, `en.json missing keys: ${missing.join(', ')}`).toEqual([]);
  });
});
