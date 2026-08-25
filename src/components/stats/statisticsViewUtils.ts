import { Frown, Meh, Smile, Heart } from 'lucide-react';
import type { LucideIcon } from 'lucide-react';
import type { CSSProperties } from 'react';
import type {
  Formatter,
  NameType,
  ValueType,
} from 'recharts/types/component/DefaultTooltipContent';
import type { CoreStatistics, MoodDistribution, MoodValue } from '../../types/api';
import { getMoodIcon } from '../../utils/moodUtils';
import { entryDateKey, toISODateKey } from '../../utils/dateUtils';
import { translate } from '../../i18n';
import type { I18nKey } from '../../i18n';

export type RangeOption = 7 | 30 | 90;

export const DEFAULT_RANGE: RangeOption = 7;

export const RANGE_OPTIONS: readonly RangeOption[] = Object.freeze([DEFAULT_RANGE, 30, 90]);

export const TOOLTIP_STYLE: Readonly<CSSProperties> = Object.freeze({
  backgroundColor: 'var(--bg-card)',
  border: '1px solid var(--border)',
  borderRadius: '8px',
  boxShadow: 'var(--shadow-md)',
});

export const DEFAULT_METRICS: Readonly<Pick<CoreStatistics, 'total_entries' | 'average_mood'>> =
  Object.freeze({ total_entries: 0, average_mood: 0 });
export const EMPTY_OBJECT: MoodDistribution = Object.freeze({});
export const MIN_TAG_OCCURRENCES = 2;

export interface MoodLegendEntry {
  value: MoodValue;
  icon: LucideIcon;
  color: string;
  label: string;
  shorthand: string;
}

// `label`/`shorthand` are getters (not baked-in strings) so a hot-swapped
// language pack is picked up on next render instead of freezing English at
// module-load time — `translate()` always reads the currently active
// translator (see src/i18n/index.tsx).
export const MOOD_LEGEND: readonly MoodLegendEntry[] = Object.freeze([
  { value: 1, icon: Frown, color: 'var(--mood-1)', get label() { return translate('moods.1.label'); }, get shorthand() { return translate('moods.1.short'); } },
  { value: 2, icon: Frown, color: 'var(--mood-2)', get label() { return translate('moods.2.label'); }, get shorthand() { return translate('moods.2.short'); } },
  { value: 3, icon: Meh, color: 'var(--mood-3)', get label() { return translate('moods.3.label'); }, get shorthand() { return translate('moods.3.short'); } },
  { value: 4, icon: Smile, color: 'var(--mood-4)', get label() { return translate('moods.4.label'); }, get shorthand() { return translate('moods.4.short'); } },
  { value: 5, icon: Heart, color: 'var(--mood-5)', get label() { return translate('moods.5.label'); }, get shorthand() { return translate('moods.5.short'); } },
]);

// Matches the pre-i18n MOOD_FULL_LABELS/MOOD_SHORTHANDS behavior exactly:
// String(value) mirrors JS property-key coercion for the numeric mood
// values recharts passes, so only the literal keys "1".."5" resolve.
const MOOD_VALUE_KEYS: ReadonlySet<string> = new Set(['1', '2', '3', '4', '5']);

/** Full mood label ("Terrible".."Amazing") for a numeric/string mood value; '' when unrecognised. */
export const moodFullLabel = (value: unknown): string => {
  const key = String(value);
  return MOOD_VALUE_KEYS.has(key) ? translate(`moods.${key}.label` as I18nKey) : '';
};

/** Single-letter mood shorthand ("T".."A") for a numeric/string mood value; '' when unrecognised. */
export const moodShorthand = (value: unknown): string => {
  const key = String(value);
  return MOOD_VALUE_KEYS.has(key) ? translate(`moods.${key}.short` as I18nKey) : '';
};

const WEEKDAY_KEYS = ['sun', 'mon', 'tue', 'wed', 'thu', 'fri', 'sat'] as const;

/** Short weekday label ("Sun".."Sat") for a 0 (Sunday) - 6 (Saturday) index. */
export const weekdayLabel = (weekday: number): string => {
  const key = WEEKDAY_KEYS[weekday];
  return key ? translate(`common.day.${key}` as I18nKey) : '';
};

const rollingLabelFor = (dataKey: string): string | undefined => {
  if (dataKey === 'avg7') return translate('stats.avg7');
  if (dataKey === 'avg30') return translate('stats.avg30');
  return undefined;
};

export const formatTrendTooltip: Formatter<ValueType, NameType> = (value, _name, props) => {
  const rollingLabel = props?.dataKey != null ? rollingLabelFor(String(props.dataKey)) : undefined;
  if (rollingLabel) {
    if (value == null || (typeof value === 'number' && Number.isNaN(value))) {
      return [translate('stats.noData'), rollingLabel];
    }
    return [Number(value).toFixed(2), rollingLabel];
  }

  if (value == null) {
    return [translate('stats.noEntry'), translate('stats.mood')];
  }

  const label = moodFullLabel(value);
  return [label, translate('stats.mood')];
};

// ISO day key for calendar bucketing. Stored date strings go through
// entryDateKey (never new Date('YYYY-MM-DD'), which parses as UTC midnight
// and can shift the day in negative-offset timezones).
export const normalizeDateKey = (date: string | Date | null | undefined): string | null => {
  if (!date) return null;
  if (date instanceof Date) {
    return Number.isNaN(date.getTime()) ? null : toISODateKey(date);
  }
  return entryDateKey(date) || null;
};

export interface MoodDistributionDatum {
  key: MoodValue;
  label: string;
  mood: string;
  count: number;
  fill: string;
}

export const buildMoodDistributionData = (
  moodDistribution: MoodDistribution | null | undefined,
): MoodDistributionDatum[] =>
  MOOD_LEGEND.map(({ value, label, shorthand, color }) => ({
    key: value,
    label,
    mood: shorthand,
    // mood_distribution is STRING-keyed ("1".."5"); String(value) of a MoodValue is always a valid key.
    count: moodDistribution?.[String(value) as keyof MoodDistribution] ?? 0,
    fill: color,
  }));

/** Minimal entry shape aggregateTagStats needs; MoodEntryWithSelections satisfies it. */
export interface TagStatsEntry {
  mood: number | string;
  selections?: ReadonlyArray<{
    id: number;
    name?: string | null;
    /** Legacy selection rows carried label instead of name. */
    label?: string | null;
  }> | null;
}

export interface TagStat {
  tag: string;
  count: number;
  avgMood: number;
}

export interface TagStats {
  topPositive: TagStat[];
  topNegative: TagStat[];
  all: TagStat[];
}

export const aggregateTagStats = (
  entries: readonly TagStatsEntry[] | null | undefined,
  minOccurrences: number = MIN_TAG_OCCURRENCES,
): TagStats => {
  if (!entries?.length) {
    return { topPositive: [], topNegative: [], all: [] };
  }

  const aggregateMap = new Map<string, { tag: string; count: number; sum: number }>();

  for (const entry of entries) {
    const mood = Number(entry.mood);
    if (!entry.selections?.length) continue;

    for (const selection of entry.selections) {
      const key = selection.name || selection.label || String(selection.id);
      const aggregate = aggregateMap.get(key) ?? { tag: key, count: 0, sum: 0 };
      aggregate.count += 1;
      aggregate.sum += mood;
      aggregateMap.set(key, aggregate);
    }
  }

  const rows: TagStat[] = Array.from(aggregateMap.values()).map(({ tag, count, sum }) => ({
    tag,
    count,
    avgMood: count ? sum / count : 0,
  }));

  const ranked = rows.filter((row) => row.count >= minOccurrences).sort((a, b) => b.avgMood - a.avgMood);

  return {
    topPositive: ranked.slice(0, 5),
    topNegative: ranked.slice(-5).reverse(),
    all: rows,
  };
};

/** Minimal entry shape buildCalendarDays needs; MoodEntryWithSelections satisfies it. */
export interface CalendarEntry {
  date: string | Date;
  mood: number;
}

export interface CalendarDay {
  key: string;
  label: number;
  entry: CalendarEntry | null | undefined;
  IconComponent: LucideIcon | null;
  iconColor: string | null;
  isCurrentMonth: boolean;
  isToday: boolean;
}

export const buildCalendarDays = (
  entries: readonly CalendarEntry[] | null | undefined,
): CalendarDay[] => {
  const today = new Date();
  const todayKey = today.toDateString();
  const firstDay = new Date(today.getFullYear(), today.getMonth(), 1);
  const lastDay = new Date(today.getFullYear(), today.getMonth() + 1, 0);
  const startDate = new Date(firstDay);
  startDate.setDate(startDate.getDate() - firstDay.getDay());

  const lookup = new Map<string, CalendarEntry>();
  for (const entry of entries ?? []) {
    const key = normalizeDateKey(entry.date);
    if (key) {
      lookup.set(key, entry);
    }
  }

  const days: CalendarDay[] = [];
  const current = new Date(startDate);
  while (current <= lastDay || current.getDay() !== 0) {
    const dateKey = normalizeDateKey(current);
    const entry = dateKey ? lookup.get(dateKey) : null;
    const moodInfo = entry ? getMoodIcon(entry.mood) : null;

    days.push({
      key: current.toISOString(),
      label: current.getDate(),
      entry,
      IconComponent: moodInfo?.icon ?? null,
      iconColor: moodInfo?.color ?? null,
      isCurrentMonth: current.getMonth() === today.getMonth(),
      isToday: current.toDateString() === todayKey,
    });

    current.setDate(current.getDate() + 1);
  }

  return days;
};

export interface OverviewCard {
  key: string;
  value: string | number;
  label: string;
  tone: 'default' | 'danger';
}

export interface OverviewCardInput {
  totalEntries: number;
  averageMood: number | null | undefined;
  currentStreak: number;
  bestDayCount: number;
}

export const buildOverviewCards = ({
  totalEntries,
  averageMood,
  currentStreak,
  bestDayCount,
}: OverviewCardInput): OverviewCard[] => [
  {
    key: 'totalEntries',
    value: totalEntries,
    label: translate('stats.totalEntries'),
    tone: 'default',
  },
  {
    key: 'averageMood',
    value: typeof averageMood === 'number' ? averageMood.toFixed(1) : averageMood ?? '0.0',
    label: translate('stats.averageMood'),
    tone: 'default',
  },
  {
    key: 'currentStreak',
    value: currentStreak,
    label: translate('stats.currentStreak'),
    tone: 'danger',
  },
  {
    key: 'bestDay',
    value: bestDayCount,
    label: translate('stats.bestDay'),
    tone: 'default',
  },
];
