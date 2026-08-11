import { useEffect, useMemo, useRef, useState } from 'react';
import {
  BarChart,
  Bar,
  XAxis,
  YAxis,
  CartesianGrid,
  Tooltip,
  ResponsiveContainer,
} from 'recharts';
import { TrendingUp, TrendingDown, Minus } from 'lucide-react';
import Skeleton from '../ui/Skeleton';
import { TOOLTIP_STYLE, MOOD_SHORTHANDS, MOOD_LEGEND } from './statisticsViewUtils';
import {
  MONTH_NAMES_SHORT,
  MONTH_NAMES_FULL,
  buildWeekdayChartData,
  buildHeatmapGrid,
  splitCorrelationRows,
  moodColorVar,
  formatAvg,
  formatSignedDelta,
} from './extendedStatsUtils';

const CURRENT_YEAR = new Date().getFullYear();
const MIN_HEATMAP_YEAR = 1970;

const SectionNote = ({ tone = 'muted', children }) => (
  <div
    className={`statistics-view__section-note${
      tone === 'error' ? ' statistics-view__section-note--error' : ''
    }`}
  >
    {children}
  </div>
);

const formatWeekdayTooltip = (value, _name, props) => [
  `${Number(value).toFixed(2)} avg · ${props.payload.count} ${
    props.payload.count === 1 ? 'entry' : 'entries'
  }`,
  props.payload.day,
];

// Average mood per weekday. Single measure, single hue; entry counts ride
// along in the tooltip so no average appears without its sample size.
export const WeekdayPatternSection = ({ weekdayAverages, loading, error }) => {
  const chartData = useMemo(
    () => buildWeekdayChartData(weekdayAverages),
    [weekdayAverages],
  );
  const hasData = chartData.some((row) => row.count > 0);

  return (
    <div className="statistics-view__card statistics-view__section" id="weekday-pattern">
      <div className="statistics-view__section-header">
        <h3 className="statistics-view__section-title">Weekday Pattern</h3>
      </div>

      {loading && <Skeleton height={220} radius={12} />}
      {!loading && error && <SectionNote tone="error">{error}</SectionNote>}
      {!loading && !error && !hasData && (
        <SectionNote>No entries yet — averages will appear per weekday.</SectionNote>
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
              tickFormatter={(value) => MOOD_SHORTHANDS[value] || ''}
            />
            <Tooltip contentStyle={TOOLTIP_STYLE} formatter={formatWeekdayTooltip} />
            <Bar dataKey="avg" fill="var(--accent-600)" radius={[4, 4, 0, 0]} />
          </BarChart>
        </ResponsiveContainer>
      )}
    </div>
  );
};

const CorrelationRow = ({ row, withLabel, withoutLabel }) => {
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
          {formatAvg(row.avgWith)} avg over {row.countWith}
          {withLabel ? ` ${withLabel}` : ''} / {formatAvg(row.avgWithout)} over{' '}
          {row.countWithout} {withoutLabel}
        </span>
      </div>
    </div>
  );
};

// Shared with/without comparison list — used for both tag and goal
// correlations. Rows where either side has fewer than 3 entries hide behind
// the "show small samples" toggle; counts are always printed on both sides.
export const CorrelationSection = ({
  title,
  rows,
  withLabel = '',
  withoutLabel = 'without',
  emptyLabel,
  note,
  loading,
  error,
}) => {
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
                ? 'Hide small samples'
                : `Show small samples (${smallSample.length})`}
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
                withoutLabel={withoutLabel}
              />
            ))}
            {main.length === 0 && !showSmallSamples && (
              <SectionNote>
                Only small samples so far — use the toggle to see them.
              </SectionNote>
            )}
            {showSmallSamples &&
              smallSample.map((row) => (
                <CorrelationRow
                  key={row.id}
                  row={row}
                  withLabel={withLabel}
                  withoutLabel={withoutLabel}
                />
              ))}
          </div>
          <div className="statistics-view__tag-note">{note}</div>
        </>
      )}
    </div>
  );
};

const HEATMAP_DAY_LABELS = [
  { weekday: 1, label: 'Mon' },
  { weekday: 3, label: 'Wed' },
  { weekday: 5, label: 'Fri' },
];

const heatmapCellTitle = (cell) =>
  cell.count > 0
    ? `${cell.label} — avg mood ${formatAvg(cell.mood, 1)} (${cell.count} ${
        cell.count === 1 ? 'entry' : 'entries'
      })`
    : `${cell.label} — no entry`;

// GitHub-style year grid built from plain divs: weeks as columns, weekdays
// as rows, cell color from the app's mood scale, gray for unlogged days.
export const HeatmapSection = ({ heatmap, year, onYearChange, loading, error }) => {
  const grid = useMemo(() => buildHeatmapGrid(year, heatmap?.days), [year, heatmap]);
  const scrollRef = useRef(null);
  // Tracks the year we've already auto-scrolled, so re-renders (loading
  // toggles, unrelated prop churn) don't keep yanking a manual scroll back —
  // only a genuine year change re-snaps.
  const snappedYearRef = useRef(null);

  // Initial scroll position: land on the current month instead of January,
  // so the horizontally-scrolled (overflow-x: auto) heatmap doesn't force a
  // long swipe to see anything recent. Only applies to the current year —
  // past years open at their natural start (scrollLeft 0).
  useEffect(() => {
    const scrollEl = scrollRef.current;
    if (!scrollEl || year !== CURRENT_YEAR || snappedYearRef.current === year) return;
    const currentMonthLabel = MONTH_NAMES_SHORT[new Date().getMonth()];
    const target = scrollEl.querySelector(`[data-month="${currentMonthLabel}"]`);
    if (!target) return;
    scrollEl.scrollLeft = Math.max(0, target.offsetLeft - 8);
    snappedYearRef.current = year;
  }, [grid, year]);

  return (
    <div className="statistics-view__card statistics-view__section" id="mood-heatmap">
      <div className="statistics-view__section-header">
        <h3 className="statistics-view__section-title">Year in Moods</h3>
        <div className="statistics-view__button-row">
          <button
            type="button"
            className="statistics-view__ghost-button"
            aria-label="Previous year"
            disabled={year <= MIN_HEATMAP_YEAR}
            onClick={() => onYearChange(year - 1)}
          >
            ‹
          </button>
          <span className="statistics-view__heatmap-year">{year}</span>
          <button
            type="button"
            className="statistics-view__ghost-button"
            aria-label="Next year"
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
              {HEATMAP_DAY_LABELS.map(({ weekday, label }) => (
                <span
                  key={label}
                  className="statistics-view__heatmap-day-label"
                  style={{ gridColumn: 1, gridRow: weekday + 2 }}
                >
                  {label}
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
              No entry
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
            {heatmap?.days_logged ?? 0} {heatmap?.days_logged === 1 ? 'day' : 'days'}{' '}
            logged in {year} · cell color = average mood that day
          </div>
        </>
      )}
    </div>
  );
};

const DigestStat = ({ value, label }) => (
  <div className="statistics-view__digest-stat">
    <div className="statistics-view__digest-value">{value}</div>
    <div className="statistics-view__overview-label">{label}</div>
  </div>
);

// Current-month digest: entries, average, trend vs the previous month,
// top tags (with selection counts), longest streak.
export const DigestSection = ({ digest, loading, error }) => {
  const monthName = digest ? MONTH_NAMES_FULL[digest.month - 1] : '';
  const prevMonthName = digest ? MONTH_NAMES_SHORT[(digest.month + 10) % 12] : '';

  let TrendIcon = Minus;
  let trendColor = 'var(--text-muted)';
  let trendText = 'no previous-month data';
  if (digest?.mood_trend != null) {
    if (digest.mood_trend > 0) {
      TrendIcon = TrendingUp;
      trendColor = 'var(--mood-4)';
    } else if (digest.mood_trend < 0) {
      TrendIcon = TrendingDown;
      trendColor = 'var(--mood-1)';
    }
    trendText = `${formatSignedDelta(digest.mood_trend)} vs ${prevMonthName}`;
  }

  return (
    <div className="statistics-view__card statistics-view__section" id="monthly-digest">
      <div className="statistics-view__section-header">
        <h3 className="statistics-view__section-title">
          {digest ? `${monthName} ${digest.year} Digest` : 'Monthly Digest'}
        </h3>
      </div>

      {loading && <Skeleton height={120} radius={12} />}
      {!loading && error && <SectionNote tone="error">{error}</SectionNote>}
      {!loading && !error && digest && (
        <>
          <div className="statistics-view__digest-grid">
            <DigestStat
              value={digest.entries_logged}
              label={digest.entries_logged === 1 ? 'Entry logged' : 'Entries logged'}
            />
            <DigestStat
              value={formatAvg(digest.average_mood)}
              label={`Average mood (${digest.entries_logged} ${
                digest.entries_logged === 1 ? 'entry' : 'entries'
              })`}
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
              label={`Trend · ${trendText}`}
            />
            <DigestStat
              value={digest.longest_streak}
              label={`Longest streak (${
                digest.longest_streak === 1 ? 'day' : 'days'
              })`}
            />
          </div>

          {digest.entries_logged === 0 && (
            <SectionNote>No entries logged this month yet.</SectionNote>
          )}
          {digest.top_tags?.length > 0 && (
            <div className="statistics-view__digest-tags">
              {digest.top_tags.map((tag) => (
                <span
                  key={tag.option_id}
                  className="statistics-view__digest-chip"
                  title={tag.group_name}
                >
                  {tag.option_name} ×{tag.times_selected}
                </span>
              ))}
            </div>
          )}
        </>
      )}
    </div>
  );
};
