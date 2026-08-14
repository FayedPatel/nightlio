import { describe, expect, it } from 'vitest';
import { normalizeDateKey } from './statisticsViewUtils';

describe('normalizeDateKey', () => {
  it('keeps an ISO string on its own day (no UTC midnight shift)', () => {
    expect(normalizeDateKey('2026-08-13')).toBe('2026-08-13');
  });

  it('normalises legacy US locale strings to the same key', () => {
    expect(normalizeDateKey('8/13/2026')).toBe(normalizeDateKey('2026-08-13'));
  });

  it('keys a Date instance by its local day', () => {
    expect(normalizeDateKey(new Date(2026, 7, 13, 23, 30))).toBe('2026-08-13');
  });

  it('returns null for empty values and invalid Dates', () => {
    expect(normalizeDateKey(null)).toBeNull();
    expect(normalizeDateKey('')).toBeNull();
    expect(normalizeDateKey(new Date('nonsense'))).toBeNull();
  });
});
