import { WEEK_DAYS } from './statisticsViewUtils';
import type { OverviewCard } from './statisticsViewUtils';
import type {
  GoalCorrelation,
  HeatmapDay,
  MoodVolatility,
  RollingAveragePoint,
  TagCorrelation,
  WeekdayAverage,
} from '../../types/api';

// Below this many entries on either side, a correlation row is considered a
// small sample and hidden behind the "show small samples" toggle.
export const MIN_CORRELATION_SAMPLE = 3;

export const MONTH_NAMES_SHORT: readonly string[] = [
  'Jan', 'Feb', 'Mar', 'Apr', 'May', 'Jun',
  'Jul', 'Aug', 'Sep', 'Oct', 'Nov', 'Dec',
];

export const MONTH_NAMES_FULL: readonly string[] = [
  'January', 'February', 'March', 'April', 'May', 'June',
  'July', 'August', 'September', 'October', 'November', 'December',
];

// Canonical implementation moved to utils/dateUtils; re-exported here so
// existing imports keep working.
import { toISODateKey } from '../../utils/dateUtils';
export { toISODateKey };

export const formatAvg = (value: number | null | undefined, digits = 2): string =>
  value == null || Number.isNaN(value) ? '—' : Number(value).toFixed(digits);

// Maps a fractional average mood to the app's mood scale color token.
export const moodColorVar = (averageMood: number | null | undefined): string | null => {
  if (averageMood == null || Number.isNaN(averageMood)) return null;
  const level = Math.min(5, Math.max(1, Math.round(averageMood)));
  return `var(--mood-${level})`;
};

export interface RollingOverlayPoint {
  avg7: number | null;
  avg30: number | null;
}

// Aligns the server rolling-average series (ISO-dated, logged days only) with
// the trend chart's window of the last `days` calendar days ending today —
// the same day sequence getWeeklyMoodData() generates. Days the server has
// no row for come back null so recharts can bridge them with connectNulls.
export const buildRollingOverlay = (
  series: readonly RollingAveragePoint[] | null | undefined,
  days: number,
): RollingOverlayPoint[] => {
  const byDate = new Map<string, RollingAveragePoint>();
  for (const point of series ?? []) {
    byDate.set(point.date, point);
  }
  const overlay: RollingOverlayPoint[] = [];
  const today = new Date();
  for (let i = days - 1; i >= 0; i--) {
    const date = new Date(today);
    date.setDate(date.getDate() - i);
    const point = byDate.get(toISODateKey(date));
    overlay.push({
      avg7: point?.rolling_7 ?? null,
      avg30: point?.rolling_30 ?? null,
    });
  }
  return overlay;
};

export interface WeekdayChartDatum {
  day: string;
  avg: number | null;
  count: number;
}

// Shapes the always-7-row weekday_averages payload (0 = Sunday) for the bar
// chart; `avg` stays null on weekdays with no entries so no bar renders.
export const buildWeekdayChartData = (
  weekdayAverages: readonly WeekdayAverage[] | null | undefined,
): WeekdayChartDatum[] =>
  (weekdayAverages ?? []).map((row) => ({
    day: WEEK_DAYS[row.weekday] ?? row.name,
    avg: row.average_mood,
    count: row.entry_count ?? 0,
  }));

// Overview tile for mood volatility (trailing 30-day sample stddev). The
// backend sends stddev: null when fewer than 2 entries fall in the window;
// that renders as an em dash, per design.
export const buildVolatilityCard = (
  volatility: MoodVolatility | null | undefined,
  loading = false,
): OverviewCard => {
  const stddev = volatility?.stddev;
  const count = volatility?.entry_count ?? 0;
  const windowDays = volatility?.window_days ?? 30;
  return {
    key: 'volatility',
    value: loading ? '…' : stddev != null ? stddev.toFixed(2) : '—',
    label: `Volatility (${windowDays}d · ${count} ${count === 1 ? 'entry' : 'entries'})`,
    tone: 'default',
  };
};

// Common row shape both correlation lists share.
export interface CorrelationRowData {
  id: string;
  name: string;
  detail: string | null;
  avgWith: number | null;
  countWith: number;
  avgWithout: number | null;
  countWithout: number;
}

export const normalizeTagCorrelations = (
  rows: readonly TagCorrelation[] | null | undefined,
): CorrelationRowData[] =>
  (rows ?? []).map((row) => ({
    id: `tag-${row.option_id}`,
    name: row.option_name,
    detail: row.group_name,
    avgWith: row.average_mood_selected,
    countWith: row.entry_count_selected ?? 0,
    avgWithout: row.average_mood_not_selected,
    countWithout: row.entry_count_not_selected ?? 0,
  }));

export const normalizeGoalCorrelations = (
  rows: readonly GoalCorrelation[] | null | undefined,
): CorrelationRowData[] =>
  (rows ?? []).map((row) => ({
    id: `goal-${row.goal_id}`,
    name: row.goal_name,
    detail: null,
    avgWith: row.average_mood_completed,
    countWith: row.entry_count_completed ?? 0,
    avgWithout: row.average_mood_not_completed,
    countWithout: row.entry_count_not_completed ?? 0,
  }));

export type CorrelationRowWithDiff = CorrelationRowData & { diff: number | null };

const byAbsoluteImpact = (a: CorrelationRowWithDiff, b: CorrelationRowWithDiff): number => {
  if (a.diff == null && b.diff == null) return 0;
  if (a.diff == null) return 1;
  if (b.diff == null) return -1;
  return Math.abs(b.diff) - Math.abs(a.diff);
};

export interface SplitCorrelationRows {
  main: CorrelationRowWithDiff[];
  smallSample: CorrelationRowWithDiff[];
}

// Splits normalized correlation rows into confident rows and small samples
// (either side under MIN_CORRELATION_SAMPLE entries), both sorted by the
// absolute with/without difference so the biggest signal sits on top.
export const splitCorrelationRows = (
  rows: readonly CorrelationRowData[] | null | undefined,
  minCount: number = MIN_CORRELATION_SAMPLE,
): SplitCorrelationRows => {
  const withDiff: CorrelationRowWithDiff[] = (rows ?? []).map((row) => ({
    ...row,
    diff:
      row.avgWith != null && row.avgWithout != null
        ? row.avgWith - row.avgWithout
        : null,
  }));
  return {
    main: withDiff
      .filter((row) => row.countWith >= minCount && row.countWithout >= minCount)
      .sort(byAbsoluteImpact),
    smallSample: withDiff
      .filter((row) => row.countWith < minCount || row.countWithout < minCount)
      .sort(byAbsoluteImpact),
  };
};

export interface HeatmapCell {
  iso: string;
  weekIndex: number;
  weekday: number;
  mood: number | null;
  count: number;
  label: string;
}

export interface HeatmapMonthLabel {
  weekIndex: number;
  label: string;
}

export interface HeatmapGrid {
  weeks: number;
  cells: HeatmapCell[];
  monthLabels: HeatmapMonthLabel[];
}

// Lays out a GitHub-style year grid: weeks as columns, weekdays (0 = Sunday)
// as rows. `days` is the heatmap payload (logged days only); unlogged days
// come back with mood: null so they render as the neutral cell color.
export const buildHeatmapGrid = (
  year: number,
  days: readonly HeatmapDay[] | null | undefined,
): HeatmapGrid => {
  const lookup = new Map<string, HeatmapDay>();
  for (const day of days ?? []) {
    lookup.set(day.date, day);
  }

  const first = new Date(year, 0, 1);
  const startOffset = first.getDay();
  const cells: HeatmapCell[] = [];
  const monthLabels: HeatmapMonthLabel[] = [];

  const cursor = new Date(first);
  let index = 0;
  while (cursor.getFullYear() === year) {
    const weekIndex = Math.floor((startOffset + index) / 7);
    if (cursor.getDate() === 1) {
      monthLabels.push({ weekIndex, label: MONTH_NAMES_SHORT[cursor.getMonth()] ?? '' });
    }
    const iso = toISODateKey(cursor);
    const logged = lookup.get(iso);
    cells.push({
      iso,
      weekIndex,
      weekday: cursor.getDay(),
      mood: logged?.average_mood ?? null,
      count: logged?.entry_count ?? 0,
      label: `${MONTH_NAMES_SHORT[cursor.getMonth()]} ${cursor.getDate()}, ${year}`,
    });
    cursor.setDate(cursor.getDate() + 1);
    index += 1;
  }

  return {
    weeks: Math.ceil((startOffset + index) / 7),
    cells,
    monthLabels,
  };
};

export const formatSignedDelta = (
  value: number | null | undefined,
  digits = 2,
): string | null => {
  if (value == null || Number.isNaN(value)) return null;
  const fixed = Number(value).toFixed(digits);
  return value > 0 ? `+${fixed}` : fixed;
};
