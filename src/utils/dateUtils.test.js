import { describe, expect, it } from 'vitest';
import {
  entryDateKey,
  formatEntryDate,
  toISODateKey,
  todayISO,
  yesterdayISO,
} from './dateUtils';

describe('toISODateKey', () => {
  it('formats a local date with zero padding', () => {
    expect(toISODateKey(new Date(2026, 0, 5))).toBe('2026-01-05');
  });

  it('stays on the local day near midnight (no UTC shift)', () => {
    expect(toISODateKey(new Date(2026, 7, 13, 23, 30))).toBe('2026-08-13');
  });
});

describe('todayISO / yesterdayISO', () => {
  it('yesterday is exactly one day before today', () => {
    const [ty, tm, td] = todayISO().split('-').map(Number);
    const today = new Date(ty, tm - 1, td);
    const [yy, ym, yd] = yesterdayISO().split('-').map(Number);
    const yesterday = new Date(yy, ym - 1, yd);
    expect(today - yesterday).toBe(24 * 60 * 60 * 1000);
  });
});

describe('entryDateKey', () => {
  it('normalises ISO dates, padding as needed', () => {
    expect(entryDateKey('2026-08-13')).toBe('2026-08-13');
    expect(entryDateKey('2026-8-3')).toBe('2026-08-03');
  });

  it('normalises legacy US locale dates', () => {
    expect(entryDateKey('8/13/2026')).toBe('2026-08-13');
    expect(entryDateKey('12/1/2026')).toBe('2026-12-01');
  });

  it('maps both shapes of the same day to the same key', () => {
    expect(entryDateKey('8/13/2026')).toBe(entryDateKey('2026-08-13'));
  });

  it('passes unrecognised strings through and rejects non-strings', () => {
    expect(entryDateKey('not a date')).toBe('not a date');
    expect(entryDateKey(null)).toBe('');
    expect(entryDateKey(undefined)).toBe('');
  });
});

describe('formatEntryDate', () => {
  it('renders both stored shapes identically', () => {
    expect(formatEntryDate('2026-08-13')).toBe(formatEntryDate('8/13/2026'));
  });

  it('falls back to the raw value when unparseable', () => {
    expect(formatEntryDate('mystery')).toBe('mystery');
    expect(formatEntryDate(null)).toBe('');
  });
});
