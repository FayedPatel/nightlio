import { describe, expect, it } from 'vitest';
import { getMoodLabel, getWeeklyMoodData, movingAverage } from './moodUtils';
import { todayISO, yesterdayISO } from './dateUtils';

describe('getMoodLabel', () => {
  it('maps known values and falls back for unknown ones', () => {
    expect(getMoodLabel(1)).toBe('Terrible');
    expect(getMoodLabel(5)).toBe('Amazing');
    expect(getMoodLabel(42)).toBe('Unknown');
  });
});

describe('getWeeklyMoodData', () => {
  it('finds entries stored in either date shape', () => {
    const [year, month, day] = yesterdayISO().split('-').map(Number);
    const legacyYesterday = `${month}/${day}/${year}`;

    const week = getWeeklyMoodData([
      { date: todayISO(), mood: 4 },
      { date: legacyYesterday, mood: 2 },
    ]);

    expect(week).toHaveLength(7);
    expect(week[6]?.mood).toBe(4); // today, ISO shape
    expect(week[5]?.mood).toBe(2); // yesterday, legacy shape
    expect(week.filter((d) => d.hasEntry)).toHaveLength(2);
  });

  it('marks days without entries as gaps', () => {
    const week = getWeeklyMoodData([]);
    expect(week.every((d) => d.mood === null && !d.hasEntry)).toBe(true);
  });
});

describe('movingAverage', () => {
  it('averages over the trailing window, skipping nulls', () => {
    expect(movingAverage([2, 4], 7)).toEqual([2, 3]);
    expect(movingAverage([2, null, 4], 7)).toEqual([2, 2, 3]);
    expect(movingAverage([], 7)).toEqual([]);
  });
});
