import { Frown, Meh, Smile, Heart } from 'lucide-react';
import type { LucideIcon } from 'lucide-react';
import type { MoodEntry, MoodValue } from '../types/api';
import { entryDateKey, formatEntryDate, toISODateKey } from './dateUtils';

// Resolve a CSS variable to its computed value (fallback to provided value)
const cssVar = (name: string, fallback: string): string => {
  try {
    const v = getComputedStyle(document.documentElement).getPropertyValue(name).trim();
    return v || fallback;
  } catch {
    return fallback;
  }
};

export interface Mood {
  /** Lucide component stored as data; render via `const Icon = mood.icon`. */
  icon: LucideIcon;
  value: MoodValue;
  /** CSS custom-property reference, e.g. 'var(--mood-1)'. */
  color: string;
  label: string;
  /** Mood-music vibe tag passed to GET /api/music/vibe. */
  tag: string;
}

export const MOODS: Mood[] = [
  { icon: Frown, value: 1, color: 'var(--mood-1)', label: 'Terrible', tag: 'dark+ambient' },
  { icon: Frown, value: 2, color: 'var(--mood-2)', label: 'Bad', tag: 'melancholic' },
  { icon: Meh,   value: 3, color: 'var(--mood-3)', label: 'Okay', tag: 'lofi+chill' },
  { icon: Smile, value: 4, color: 'var(--mood-4)', label: 'Good', tag: 'upbeat+pop' },
  { icon: Heart, value: 5, color: 'var(--mood-5)', label: 'Amazing', tag: 'synthwave+energy' },
];

/**
 * Deliberately narrower than Mood: the not-found fallback branch has no
 * label/tag to offer, so only { icon, color } is part of this contract.
 * `color` is always a resolved concrete value, never a var() reference.
 */
export interface MoodIconInfo {
  icon: LucideIcon;
  color: string;
}

export const getMoodIcon = (moodValue: number): MoodIconInfo => {
  const mood = MOODS.find(m => m.value === moodValue);
  if (!mood) return { icon: Meh, color: cssVar('--mood-3', '#f1fa8c') };
  // Resolve CSS var to concrete color for places that need an actual color value
  const resolved = mood.color.startsWith('var(')
    ? cssVar(mood.color.slice(4, -1), '#999')
    : mood.color;
  return { icon: mood.icon, color: resolved };
};

export const getMoodLabel = (moodValue: number): string => {
  const mood = MOODS.find(m => m.value === moodValue);
  return mood ? mood.label : 'Unknown';
};

export const formatEntryTime = (
  entry: Pick<MoodEntry, 'date'> & Partial<Pick<MoodEntry, 'created_at'>>,
): string => {
  if (entry.created_at) {
    const date = new Date(entry.created_at);
    const time = date.toLocaleTimeString([], {
      hour: '2-digit',
      minute: '2-digit',
      hour12: true,
    });
    return `${formatEntryDate(entry.date)} at ${time}`;
  }
  return formatEntryDate(entry.date);
};

export interface WeeklyMoodPoint {
  /** Short display label ('Mon' for <=7 days, 'Aug 13' beyond). */
  date: string;
  mood: MoodValue | null;
  /**
   * Historical misnomer: despite the name this is a lucide icon COMPONENT
   * (LucideIcon), never an emoji string.
   */
  moodEmoji: LucideIcon | null;
  hasEntry: boolean;
}

export const getWeeklyMoodData = (
  pastEntries: Array<Pick<MoodEntry, 'date' | 'mood'>>,
  days = 7,
): WeeklyMoodPoint[] => {
  const today = new Date();
  const weekData: WeeklyMoodPoint[] = [];

  // Create entry lookup by normalised ISO day key so both stored date shapes
  // (M/D/YYYY and YYYY-MM-DD) land on the same chart day
  const entryLookup: Record<string, Pick<MoodEntry, 'date' | 'mood'>> = {};
  pastEntries.forEach(entry => {
    entryLookup[entryDateKey(entry.date)] = entry;
  });

  // Get last N days
  for (let i = days - 1; i >= 0; i--) {
    const date = new Date(today);
    date.setDate(date.getDate() - i);
    const entry = entryLookup[toISODateKey(date)];

    weekData.push({
      date: days <= 7
        ? date.toLocaleDateString('en-US', { weekday: 'short' })
        : date.toLocaleDateString('en-US', { month: 'short', day: 'numeric' }),
      mood: entry ? entry.mood : null,
      moodEmoji: entry ? getMoodIcon(entry.mood).icon : null,
      hasEntry: !!entry,
    });
  }

  return weekData;
};

export const movingAverage = (
  arr: Array<number | null | undefined>,
  windowSize = 7,
): Array<number | null> => {
  const res: Array<number | null> = [];
  for (let i = 0; i < arr.length; i++) {
    const start = Math.max(0, i - windowSize + 1);
    const slice = arr.slice(start, i + 1).filter((v): v is number => v != null);
    res.push(slice.length ? slice.reduce((a, b) => a + b, 0) / slice.length : null);
  }
  return res;
};
