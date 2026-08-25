import { useEffect, useMemo, useRef, useState } from 'react';
import type { ReactNode } from 'react';
import {
  BarChart,
  Bar,
  XAxis,
  YAxis,
  CartesianGrid,
  Tooltip,
  ResponsiveContainer,
} from 'recharts';
import type {
  Formatter,
  NameType,
  ValueType,
} from 'recharts/types/component/DefaultTooltipContent';
import { TrendingUp, TrendingDown, Minus } from 'lucide-react';
import type { LucideIcon } from 'lucide-react';
import Skeleton from '../ui/Skeleton';
import type { Digest, Heatmap, WeekdayAverage } from '../../types/api';
import { TOOLTIP_STYLE, moodShorthand, weekdayLabel, MOOD_LEGEND } from './statisticsViewUtils';
import {
  monthShortLabel,
  monthFullLabel,
  buildWeekdayChartData,
  buildHeatmapGrid,
  splitCorrelationRows,
  moodColorVar,
  formatAvg,
  formatSignedDelta,
} from './extendedStatsUtils';
import type { CorrelationRowData, CorrelationRowWithDiff, HeatmapCell } from './extendedStatsUtils';
import { translate, useI18n } from '../../i18n';

const CURRENT_YEAR = new Date().getFullYear();
const MIN_HEATMAP_YEAR = 1970;

interface SectionNoteProps {
  tone?: 'muted' | 'error';
  children: ReactNode;
}

const SectionNote = ({ tone = 'muted', children }: SectionNoteProps) => (
  <div
    className={`statistics-view__section-note${
      tone === 'error' ? ' statistics-view__section-note--error' : ''
    }`}
  >
    {children}
  </div>
);

// recharts Payload.payload is typed `any`; the datum here is always a WeekdayChartDatum.
const formatWeekdayTooltip: Formatter<ValueType, NameType> = (value, _name, props) => [
  translate('stats.weekdayTooltip', { avg: Number(value).toFixed(2), count: props.payload.count }),
  props.payload.day,
];

export interface WeekdayPatternSectionProps {
  weekdayAverages: WeekdayAverage[] | undefined;
  loading: boolean;
  error: string | null;
}

// Average mood per weekday. Single measure, single hue; entry counts ride
// along in the tooltip so no average appears without its sample size.
export const WeekdayPatternSection = ({ weekdayAverages, loading, error }: WeekdayPatternSectionProps) => {
  const { t } = useI18n();
  const chartData = useMemo(
    () => buildWeekdayChartData(weekdayAverages),
    [weekdayAverages],
  );
  const hasData = chartData.some((row) => row.count > 0);

  return (
    <div className="statistics-view__card statistics-view__section" id="weekday-pattern">
      <div className="statistics-view__section-header">
        <h3 className="statistics-view__section-title">{t('stats.weekdayPattern')}</h3>
      </div>

      {loading && <Skeleton height={220} radius={12} />}
      {!loading && error && <SectionNote tone="error">{error}</SectionNote>}
      {!loading && !error && !hasData && (
        <SectionNote>{t('stats.weekdayEmpty')}</SectionNote>
      )}
      {!loading && !error && hasData && (
        <ResponsiveContainer width="100%" height={220}>
          <BarChart data={chartData} margin={{ top: 10, right: 20, left: 0, bottom: 0 }}>
            <CartesianGrid strokeDasharray="3 3" stroke="var(--border)" />
            <XAxis
              dataKey="day"
              tick={{ fontSize: 12, fill: 'var(--text-muted)' }}
              axisLine={{ stroke: 'var(--border)' }}
            />
            <YAxis
              domain={[0, 5.5]}
              ticks={[1, 2, 3, 4, 5]}
              tick={{ fontSize: 12, fill: 'var(--text-muted)' }}
              axisLine={{ stroke: 'var(--border)' }}
              width={20}
              tickFormatter={(value) => moodShorthand(value)}
            />
            <Tooltip contentStyle={TOOLTIP_STYLE} formatter={formatWeekdayTooltip} />
            <Bar dataKey="avg" fill="var(--accent-600)" radius={[4, 4, 0, 0]} />
          </BarChart>
        </ResponsiveContainer>
      )}
    </div>
  );
};

interface CorrelationRowProps {
  row: CorrelationRowWithDiff;
  withLabel: string;
  withoutLabel: string;
}

const CorrelationRow = ({ row, withLabel, withoutLabel }: CorrelationRowProps) => {
  const { t } = useI18n();
  const diffColor =
    row.diff == null
      ? 'var(--text-muted)'
      : row.diff >= 0
        ? 'var(--mood-4)'
        : 'var(--mood-1)';

  return (
    <div className="statistics-view__corr-row">
      <div className="statistics-view__corr-name">
        <span>{row.name}</span>
        {row.detail && <span className="statistics-view__corr-detail">{row.detail}</span>}
      </div>
      <div className="statistics-view__corr-values">
        <span className="statistics-view__corr-diff" style={{ color: diffColor }}>
          {row.diff == null ? '—' : formatSignedDelta(row.diff)}
        </span>
        <span className="statistics-view__corr-breakdown">
          {t('stats.corrBreakdown', {
            avgWith: formatAvg(row.avgWith),
            countWith: row.countWith,
            withLabel: withLabel ? ` ${withLabel}` : '',
            avgWithout: formatAvg(row.avgWithout),
            countWithout: row.countWithout,
            withoutLabel,
          })}
        </span>
      </div>
    </div>
  );
};

export interface CorrelationSectionProps {
  title: string;
  rows: CorrelationRowData[];
  withLabel?: string;
  withoutLabel?: string;
  emptyLabel: ReactNode;
  note: ReactNode;
  loading: boolean;
  error: string | null;
}

// Shared with/without comparison list — used for both tag and goal
// correlations. Rows where either side has fewer than 3 entries hide behind
// the "show small samples" toggle; counts are always printed on both sides.
export const CorrelationSection = ({
  title,
  rows,
  withLabel = '',
  withoutLabel,
  emptyLabel,
  note,
  loading,
  error,
}: CorrelationSectionProps) => {
  const { t } = useI18n();
  const resolvedWithoutLabel = withoutLabel ?? t('stats.without');
  const [showSmallSamples, setShowSmallSamples] = useState(false);
  const { main, smallSample } = useMemo(() => splitCorrelationRows(rows), [rows]);
  const hasRows = main.length > 0 || smallSample.length > 0;

  return (
    <div className="statistics-view__card statistics-view__section">
      <div className="statistics-view__section-header">
        <h3 className="statistics-view__section-title">{title}</h3>
        <div className="statistics-view__button-row">
          {smallSample.length > 0 && (
            <button
              type="button"
              className="statistics-view__ghost-button"
              onClick={() => setShowSmallSamples((value) => !value)}
            >
              {showSmallSamples
                ? t('stats.hideSmallSamples')
                : t('stats.showSmallSamples', { count: smallSample.length })}
            </button>
          )}
        </div>
      </div>

      {loading && <Skeleton height={120} radius={12} />}
      {!loading && error && <SectionNote tone="error">{error}</SectionNote>}
      {!loading && !error && !hasRows && <SectionNote>{emptyLabel}</SectionNote>}
      {!loading && !error && hasRows && (
        <>
          <div className="statistics-view__corr-list">
            {main.map((row) => (
              <CorrelationRow
                key={row.id}
                row={row}
                withLabel={withLabel}
                withoutLabel={resolvedWithoutLabel}
              />
            ))}
            {main.length === 0 && !showSmallSamples && (
              <SectionNote>
                {t('stats.onlySmallSamples')}
              </SectionNote>
            )}
            {showSmallSamples &&
              smallSample.map((row) => (
                <CorrelationRow
                  key={row.id}
                  row={row}
                  withLabel={withLabel}
                  withoutLabel={resolvedWithoutLabel}
                />
              ))}
          </div>
          <div className="statistics-view__tag-note">{note}</div>
        </>
      )}
    </div>
  );
};

const HEATMAP_DAY_WEEKDAYS = [1, 3, 5];

const heatmapCellTitle = (cell: HeatmapCell): string =>
  cell.count > 0
    ? translate('stats.heatmapCell', { date: cell.label, avg: formatAvg(cell.mood, 1), count: cell.count })
    : translate('stats.heatmapCellEmpty', { date: cell.label });

export interface HeatmapSectionProps {
  heatmap: Heatmap | null;
  year: number;
  onYearChange: (year: number) => void;
  loading: boolean;
  error: string | null;
}

// GitHub-style year grid built from plain divs: weeks as columns, weekdays
// as rows, cell color from the app's mood scale, gray for unlogged days.
export const HeatmapSection = ({ heatmap, year, onYearChange, loading, error }: HeatmapSectionProps) => {
  const { t } = useI18n();
  const grid = useMemo(() => buildHeatmapGrid(year, heatmap?.days), [year, heatmap]);
  const scrollRef = useRef<HTMLDivElement>(null);
  // Tracks the year we've already auto-scrolled, so re-renders (loading
  // toggles, unrelated prop churn) don't keep yanking a manual scroll back —
  // only a genuine year change re-snaps.
  const snappedYearRef = useRef<number | null>(null);

  // Initial scroll position: land on the current month instead of January,
  // so the horizontally-scrolled (overflow-x: auto) heatmap doesn't force a
  // long swipe to see anything recent. Only applies to the current year —
  // past years open at their natural start (scrollLeft 0).
  useEffect(() => {
    const scrollEl = scrollRef.current;
    if (!scrollEl || year !== CURRENT_YEAR || snappedYearRef.current === year) return;
    const currentMonthLabel = monthShortLabel(new Date().getMonth() + 1);
    const target = scrollEl.querySelector<HTMLElement>(`[data-month="${currentMonthLabel}"]`);
    if (!target) return;
    scrollEl.scrollLeft = Math.max(0, target.offsetLeft - 8);
    snappedYearRef.current = year;
  }, [grid, year]);

  return (
    <div className="statistics-view__card statistics-view__section" id="mood-heatmap">
      <div className="statistics-view__section-header">
        <h3 className="statistics-view__section-title">{t('stats.yearInMoods')}</h3>
        <div className="statistics-view__button-row">
          <button
            type="button"
            className="statistics-view__ghost-button"
            aria-label={t('stats.prevYearAria')}
            disabled={year <= MIN_HEATMAP_YEAR}
            onClick={() => onYearChange(year - 1)}
          >
            ‹
          </button>
          <span className="statistics-view__heatmap-year">{year}</span>
          <button
            type="button"
            className="statistics-view__ghost-button"
            aria-label={t('stats.nextYearAria')}
            disabled={year >= CURRENT_YEAR}
            onClick={() => onYearChange(year + 1)}
          >
            ›
          </button>
        </div>
      </div>

      {loading && <Skeleton height={160} radius={12} />}
      {!loading && error && <SectionNote tone="error">{error}</SectionNote>}
      {!loading && !error && (
        <>
          <div className="statistics-view__heatmap-scroll" ref={scrollRef}>
            <div
              className="statistics-view__heatmap"
              style={{
                gridTemplateColumns: `auto repeat(${grid.weeks}, var(--stats-heatmap-cell))`,
                gridTemplateRows: 'auto repeat(7, var(--stats-heatmap-cell))',
              }}
            >
              {grid.monthLabels.map(({ weekIndex, label }) => (
                <span
                  key={label}
                  className="statistics-view__heatmap-month"
                  data-month={label}
                  style={{ gridColumn: weekIndex + 2, gridRow: 1 }}
                >
                  {label}
                </span>
              ))}
              {HEATMAP_DAY_WEEKDAYS.map((weekday) => (
                <span
                  key={weekday}
                  className="statistics-view__heatmap-day-label"
                  style={{ gridColumn: 1, gridRow: weekday + 2 }}
                >
                  {weekdayLabel(weekday)}
                </span>
              ))}
              {grid.cells.map((cell) => (
                <div
                  key={cell.iso}
                  className={`statistics-view__heatmap-cell${
                    cell.count > 0 ? ' has-mood' : ''
                  }`}
                  style={{
                    gridColumn: cell.weekIndex + 2,
                    gridRow: cell.weekday + 2,
                    background: moodColorVar(cell.mood) ?? undefined,
                  }}
                  role="img"
                  aria-label={heatmapCellTitle(cell)}
                  title={heatmapCellTitle(cell)}
                />
              ))}
            </div>
          </div>

          <div className="statistics-view__heatmap-legend">
            <span className="statistics-view__heatmap-legend-item">
              <span className="statistics-view__heatmap-swatch" />
              {t('stats.noEntry')}
            </span>
            {MOOD_LEGEND.map(({ value, color, label }) => (
              <span key={value} className="statistics-view__heatmap-legend-item">
                <span
                  className="statistics-view__heatmap-swatch"
                  style={{ background: color }}
                />
                {label}
              </span>
            ))}
          </div>
          <div className="statistics-view__tag-note">
            {t('stats.heatmapFooter', { count: heatmap?.days_logged ?? 0, year })}
          </div>
        </>
      )}
    </div>
  );
};

interface DigestStatProps {
  value: ReactNode;
  label: ReactNode;
}

const DigestStat = ({ value, label }: DigestStatProps) => (
  <div className="statistics-view__digest-stat">
    <div className="statistics-view__digest-value">{value}</div>
    <div className="statistics-view__overview-label">{label}</div>
  </div>
);

export interface DigestSectionProps {
  digest: Digest | undefined;
  loading: boolean;
  error: string | null;
}

// Current-month digest: entries, average, trend vs the previous month,
// top tags (with selection counts), longest streak.
export const DigestSection = ({ digest, loading, error }: DigestSectionProps) => {
  const { t } = useI18n();
  const monthName = digest ? monthFullLabel(digest.month) : '';
  const prevMonthName = digest ? monthShortLabel(((digest.month + 10) % 12) + 1) : '';

  let TrendIcon: LucideIcon = Minus;
  let trendColor = 'var(--text-muted)';
  let trendText = t('stats.trendNoData');
  if (digest?.mood_trend != null) {
    if (digest.mood_trend > 0) {
      TrendIcon = TrendingUp;
      trendColor = 'var(--mood-4)';
    } else if (digest.mood_trend < 0) {
      TrendIcon = TrendingDown;
      trendColor = 'var(--mood-1)';
    }
    trendText = t('stats.trendVs', { delta: formatSignedDelta(digest.mood_trend) ?? '', month: prevMonthName });
  }

  return (
    <div className="statistics-view__card statistics-view__section" id="monthly-digest">
      <div className="statistics-view__section-header">
        <h3 className="statistics-view__section-title">
          {digest ? t('stats.digestTitle', { month: monthName, year: digest.year }) : t('stats.monthlyDigest')}
        </h3>
      </div>

      {loading && <Skeleton height={120} radius={12} />}
      {!loading && error && <SectionNote tone="error">{error}</SectionNote>}
      {!loading && !error && digest && (
        <>
          <div className="statistics-view__digest-grid">
            <DigestStat
              value={digest.entries_logged}
              label={t('stats.entriesLogged', { count: digest.entries_logged })}
            />
            <DigestStat
              value={formatAvg(digest.average_mood)}
              label={t('stats.averageMoodEntries', { count: digest.entries_logged })}
            />
            <DigestStat
              value={
                <span
                  className="statistics-view__digest-trend"
                  style={{ color: trendColor }}
                >
                  <TrendIcon size={20} aria-hidden="true" />
                  {digest.mood_trend == null ? '—' : formatSignedDelta(digest.mood_trend)}
                </span>
              }
              label={t('stats.trendLabel', { text: trendText })}
            />
            <DigestStat
              value={digest.longest_streak}
              label={t('stats.longestStreak', { count: digest.longest_streak })}
            />
          </div>

          {digest.entries_logged === 0 && (
            <SectionNote>{t('stats.noEntriesThisMonth')}</SectionNote>
          )}
          {digest.top_tags?.length > 0 && (
            <div className="statistics-view__digest-tags">
              {digest.top_tags.map((tag) => (
                <span
                  key={tag.option_id}
                  className="statistics-view__digest-chip"
                  title={tag.group_name}
                >
                  {t('stats.digestTagTimes', { name: tag.option_name, count: tag.times_selected })}
                </span>
              ))}
            </div>
          )}
        </>
      )}
    </div>
  );
};
