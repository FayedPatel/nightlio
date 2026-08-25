import { useCallback, useMemo, useRef, useState } from 'react';
import type { ReactNode, RefObject } from 'react';
import {
  BarChart,
  Bar,
  XAxis,
  YAxis,
  CartesianGrid,
  Tooltip,
  Legend,
  ResponsiveContainer,
  LineChart,
  Line,
  Cell,
} from 'recharts';
import type {
  NameType,
  Payload,
  ValueType,
} from 'recharts/types/component/DefaultTooltipContent';
import Skeleton from '../ui/Skeleton';
import { exportSVGToPNG, exportDataToCSV } from '../../utils/exportUtils';
import type { Statistics } from '../../types/api';
import type { MoodEntryWithSelections } from '../../hooks/useMoodData';
import useStatisticsViewData from './useStatisticsViewData';
import type { TrendChartPoint } from './useStatisticsViewData';
import useExtendedStatistics from '../../hooks/useExtendedStatistics';
import {
  DEFAULT_RANGE,
  RANGE_OPTIONS,
  TOOLTIP_STYLE,
  MOOD_LEGEND,
  moodShorthand,
  weekdayLabel,
  formatTrendTooltip,
} from './statisticsViewUtils';
import { useI18n } from '../../i18n';
import type {
  CalendarDay,
  MoodDistributionDatum,
  OverviewCard,
  RangeOption,
  TagStats,
} from './statisticsViewUtils';
import {
  buildVolatilityCard,
  normalizeTagCorrelations,
  normalizeGoalCorrelations,
} from './extendedStatsUtils';
import {
  WeekdayPatternSection,
  CorrelationSection,
  HeatmapSection,
  DigestSection,
} from './ExtendedStatsSections';
import './StatisticsView.css';

const MoodLegend = () => (
  <div className="statistics-view__legend">
    {MOOD_LEGEND.map(({ value, icon, color, label }) => {
      const LegendIcon = icon;
      return (
        <div key={value} className="statistics-view__legend-item">
          <LegendIcon size={16} style={{ color }} />
          <span>{label}</span>
        </div>
      );
    })}
  </div>
);

const StatisticsOverviewGrid = ({ cards }: { cards: OverviewCard[] }) => (
  <div className="statistics-view__overview-grid">
    {cards.map(({ key, value, label, tone }) => {
      const valueClassName = tone === 'danger'
        ? 'statistics-view__overview-value statistics-view__overview-value--danger'
        : 'statistics-view__overview-value';

      return (
        <div key={key} className="statistics-view__card statistics-view__overview-card">
          <div className={valueClassName}>{value}</div>
          <div className="statistics-view__overview-label">{label}</div>
        </div>
      );
    })}
  </div>
);

const SectionHeader = ({ title, children }: { title: string; children?: ReactNode }) => (
  <div className="statistics-view__section-header">
    <h3 className="statistics-view__section-title">{title}</h3>
    <div className="statistics-view__button-row">{children}</div>
  </div>
);

interface RangeSelectorProps {
  range: RangeOption;
  onChange: (range: RangeOption) => void;
}

const RangeSelector = ({ range, onChange }: RangeSelectorProps) => {
  const { t } = useI18n();
  return (
    <div className="statistics-view__range-buttons">
      {RANGE_OPTIONS.map((option) => (
        <button
          key={option}
          type="button"
          onClick={() => onChange(option)}
          className={`statistics-view__range-button${range === option ? ' is-active' : ''}`}
        >
          {t('stats.rangeD', { count: option })}
        </button>
      ))}
    </div>
  );
};

interface MoodTrendSectionProps {
  chartData: TrendChartPoint[];
  range: RangeOption;
  onChangeRange: (range: RangeOption) => void;
  onExportPNG: () => void;
  onExportCSV: () => void;
  containerRef: RefObject<HTMLDivElement | null>;
  rollingNote: ReactNode;
}

const MoodTrendSection = ({ chartData, range, onChangeRange, onExportPNG, onExportCSV, containerRef, rollingNote }: MoodTrendSectionProps) => {
  const { t } = useI18n();
  return (
    <div ref={containerRef} className="statistics-view__card statistics-view__section" id="mood-trend">
      <SectionHeader title={t('stats.moodTrend')}>
        <RangeSelector range={range} onChange={onChangeRange} />
        <button type="button" className="statistics-view__ghost-button" onClick={onExportPNG}>
          {t('stats.exportPng')}
        </button>
        <button type="button" className="statistics-view__ghost-button" onClick={onExportCSV}>
          {t('stats.exportCsv')}
        </button>
      </SectionHeader>

      <ResponsiveContainer width="100%" height={320}>
        <LineChart data={chartData} margin={{ top: 20, right: 20, left: 0, bottom: 20 }}>
          <CartesianGrid strokeDasharray="3 3" stroke="var(--border)" />
          <XAxis dataKey="date" tick={{ fontSize: 12, fill: 'var(--text-muted)' }} axisLine={{ stroke: 'var(--border)' }} />
          <YAxis
            domain={[0.5, 5.5]}
            ticks={[1, 2, 3, 4, 5]}
            tick={{ fontSize: 12, fill: 'var(--text-muted)' }}
            axisLine={{ stroke: 'var(--border)' }}
            width={20}
            tickFormatter={(value) => moodShorthand(value)}
          />
          <Tooltip contentStyle={TOOLTIP_STYLE} formatter={formatTrendTooltip} />
          <Legend wrapperStyle={{ fontSize: 12 }} iconType="plainline" />
          <Line
            type="monotone"
            name={t('stats.mood')}
            dataKey="mood"
            stroke="var(--accent-600)"
            strokeWidth={3}
            dot={{ fill: 'var(--accent-600)', strokeWidth: 2, r: 6 }}
            connectNulls={false}
          />
          <Line
            type="monotone"
            name={t('stats.avg7')}
            dataKey="avg7"
            stroke="var(--danger)"
            strokeDasharray="6 6"
            strokeWidth={2}
            dot={false}
            connectNulls
          />
          <Line
            type="monotone"
            name={t('stats.avg30')}
            dataKey="avg30"
            stroke="var(--text-muted)"
            strokeDasharray="2 6"
            strokeWidth={2}
            dot={false}
            connectNulls
          />
        </LineChart>
      </ResponsiveContainer>

      {rollingNote && <div className="statistics-view__tag-note">{rollingNote}</div>}
      <MoodLegend />
    </div>
  );
};

interface DistributionSectionProps {
  chartData: MoodDistributionDatum[];
  onExportPNG: () => void;
  onExportCSV: () => void;
  containerRef: RefObject<HTMLDivElement | null>;
}

const DistributionSection = ({ chartData, onExportPNG, onExportCSV, containerRef }: DistributionSectionProps) => {
  const { t } = useI18n();
  return (
    <div ref={containerRef} className="statistics-view__card statistics-view__section" id="mood-distribution">
      <SectionHeader title={t('stats.moodDistribution')}>
        <button type="button" className="statistics-view__ghost-button" onClick={onExportPNG}>
          {t('stats.exportPng')}
        </button>
        <button type="button" className="statistics-view__ghost-button" onClick={onExportCSV}>
          {t('stats.exportCsv')}
        </button>
      </SectionHeader>

      <ResponsiveContainer width="100%" height={320}>
        <BarChart data={chartData} margin={{ top: 30, right: 20, left: 0, bottom: 20 }}>
          <CartesianGrid strokeDasharray="3 3" stroke="var(--border)" />
          <XAxis dataKey="mood" tick={{ fontSize: 16, fill: 'var(--text-muted)' }} axisLine={{ stroke: 'var(--border)' }} />
          <YAxis
            tick={{ fontSize: 12, fill: 'var(--text-muted)' }}
            axisLine={{ stroke: 'var(--border)' }}
            allowDecimals={false}
            domain={[0, 'dataMax + 1']}
            width={20}
          />
          <Tooltip
            contentStyle={TOOLTIP_STYLE}
            // recharts Payload.payload is typed `any`; the datum is always a MoodDistributionDatum.
            formatter={(value: ValueType, _name: NameType, props: Payload<ValueType, NameType>) => [t('stats.entriesTooltip', { count: String(value) }), props.payload.label]}
          />
          <Bar dataKey="count" radius={[4, 4, 0, 0]} label={{ position: 'top', fontSize: 12, fontWeight: 600, fill: 'var(--text)' }}>
            {chartData.map((entry) => (
              <Cell key={entry.key} fill={entry.fill} />
            ))}
          </Bar>
        </BarChart>
      </ResponsiveContainer>

      <MoodLegend />
    </div>
  );
};

interface TagListProps {
  heading: string;
  toneClass: string;
  tags: TagStats['topPositive'];
  emptyLabel: string;
  valueColor: string;
}

const TagList = ({ heading, toneClass, tags, emptyLabel, valueColor }: TagListProps) => (
  <div className="statistics-view__tag-list">
    <h4 className={`statistics-view__tag-heading ${toneClass}`}>{heading}</h4>
    {tags.length === 0 && <div className="statistics-view__tag-empty">{emptyLabel}</div>}
    {tags.map((tag) => (
      <div key={tag.tag} className="statistics-view__tag-item">
        <span>{tag.tag}</span>
        <span style={{ color: valueColor }}>
          {tag.avgMood.toFixed(2)} ({tag.count})
        </span>
      </div>
    ))}
  </div>
);

interface TagCorrelationsSectionProps {
  tagStats: TagStats;
  onExportCSV: () => void;
}

const TagCorrelationsSection = ({ tagStats, onExportCSV }: TagCorrelationsSectionProps) => {
  const { t } = useI18n();
  const hasTags = tagStats.topPositive.length > 0 || tagStats.topNegative.length > 0;
  if (!hasTags) return null;

  return (
    <div className="statistics-view__card statistics-view__section">
      <SectionHeader title={t('stats.tagCorrelations')}>
        <button type="button" className="statistics-view__ghost-button" onClick={onExportCSV}>
          {t('stats.exportCsv')}
        </button>
      </SectionHeader>

      <div className="statistics-view__tag-grid">
        <TagList
          heading={t('stats.topPositive')}
          toneClass="statistics-view__tag-heading--positive"
          tags={tagStats.topPositive}
          emptyLabel={t('stats.noTagsYet')}
          valueColor="var(--mood-4)"
        />
        <TagList
          heading={t('stats.topNegative')}
          toneClass="statistics-view__tag-heading--negative"
          tags={tagStats.topNegative}
          emptyLabel={t('stats.noTagsYet')}
          valueColor="var(--mood-1)"
        />
      </div>

      <div className="statistics-view__tag-note">
        {t('stats.tagNote')}
      </div>
    </div>
  );
};

const MoodCalendarSection = ({ days }: { days: CalendarDay[] }) => {
  const { t } = useI18n();
  return (
    <div className="statistics-view__card statistics-view__calendar-card">
      <h3 className="statistics-view__calendar-title">{t('stats.moodCalendar')}</h3>
      <div className="statistics-view__calendar-grid">
        {[0, 1, 2, 3, 4, 5, 6].map((weekday) => (
          <div key={weekday} className="statistics-view__calendar-label">
            {weekdayLabel(weekday)}
          </div>
        ))}

        {days.map(({ key, label, entry, IconComponent, iconColor, isCurrentMonth, isToday }) => (
          <div
            key={key}
            className={`statistics-view__calendar-day${entry ? ' has-entry' : ''}${isCurrentMonth ? '' : ' is-outside'}${isToday ? ' is-today' : ''}`}
            style={{
              background: entry && iconColor ? `color-mix(in oklab, ${iconColor} 18%, transparent)` : undefined,
              color: entry && iconColor ? iconColor : undefined,
            }}
          >
            {entry && IconComponent ? <IconComponent size={16} /> : label}
          </div>
        ))}
      </div>
    </div>
  );
};

const LoadingState = () => (
  <div className="statistics-view">
    <div className="statistics-view__overview-grid">
      {[1, 2, 3, 4].map((i) => (
        <Skeleton key={i} height={120} radius={12} />
      ))}
    </div>
    <Skeleton height={36} width={260} style={{ marginBottom: 12 }} />
    <Skeleton height={320} radius={16} />
  </div>
);

const ErrorState = ({ message }: { message: string }) => (
  <div className="statistics-view statistics-view__status statistics-view__status--error">{message}</div>
);

const EmptyState = () => {
  const { t } = useI18n();
  return (
    <div className="statistics-view statistics-view__status">{t('stats.noStatistics')}</div>
  );
};

interface StatisticsViewProps {
  statistics: Statistics | null;
  pastEntries: MoodEntryWithSelections[];
  loading: boolean;
  error: string | null;
}

const StatisticsView = ({ statistics, pastEntries, loading, error }: StatisticsViewProps) => {
  const { t } = useI18n();
  const [range, setRange] = useState<RangeOption>(DEFAULT_RANGE);
  const trendRef = useRef<HTMLDivElement>(null);
  const distributionRef = useRef<HTMLDivElement>(null);

  const {
    extended,
    extendedLoading,
    extendedError,
    heatmap,
    heatmapLoading,
    heatmapError,
    heatmapYear,
    setHeatmapYear,
  } = useExtendedStatistics();

  const {
    hasStatistics,
    weeklyMoodData,
    trendChartData,
    moodDistributionData,
    tagStats,
    calendarDays,
    overviewCards,
  } = useStatisticsViewData(
    statistics,
    pastEntries,
    range,
    extended?.rolling_averages?.series ?? null,
  );

  const overviewWithVolatility = useMemo(
    () => [...overviewCards, buildVolatilityCard(extended?.mood_volatility, extendedLoading)],
    [overviewCards, extended?.mood_volatility, extendedLoading],
  );

  const tagCorrelationRows = useMemo(
    () => normalizeTagCorrelations(extended?.tag_correlations),
    [extended?.tag_correlations],
  );

  const goalCorrelationRows = useMemo(
    () => normalizeGoalCorrelations(extended?.goal_correlations),
    [extended?.goal_correlations],
  );

  const handleExportTrendPNG = useCallback(() => {
    const svg = trendRef.current?.querySelector('svg');
    if (svg) {
      exportSVGToPNG(svg, `mood-trend-${range}d.png`);
    }
  }, [range]);

  const handleExportTrendCSV = useCallback(() => {
    exportDataToCSV(weeklyMoodData, ['date', 'mood'], `mood-trend-${range}d.csv`);
  }, [weeklyMoodData, range]);

  const handleExportDistributionPNG = useCallback(() => {
    const svg = distributionRef.current?.querySelector('svg');
    if (svg) {
      exportSVGToPNG(svg, 'mood-distribution.png');
    }
  }, []);

  const handleExportDistributionCSV = useCallback(() => {
    const rows = moodDistributionData.map(({ label, count }) => ({ mood: label, count }));
    exportDataToCSV(rows, ['mood', 'count'], 'mood-distribution.csv');
  }, [moodDistributionData]);

  const handleExportTagCSV = useCallback(() => {
    exportDataToCSV(tagStats.all, ['tag', 'count', 'avgMood'], 'tag-correlations.csv');
  }, [tagStats]);

  if (loading) return <LoadingState />;
  if (error) return <ErrorState message={error} />;
  if (!hasStatistics) return <EmptyState />;

  return (
    <div className="statistics-view">
      <StatisticsOverviewGrid cards={overviewWithVolatility} />
      <DigestSection
        digest={extended?.monthly_digest}
        loading={extendedLoading}
        error={extendedError}
      />
      <MoodTrendSection
        chartData={trendChartData}
        range={range}
        onChangeRange={setRange}
        onExportPNG={handleExportTrendPNG}
        onExportCSV={handleExportTrendCSV}
        containerRef={trendRef}
        rollingNote={
          extendedError
            ? t('stats.rollingUnavailable')
            : t('stats.rollingNote')
        }
      />
      <DistributionSection
        chartData={moodDistributionData}
        onExportPNG={handleExportDistributionPNG}
        onExportCSV={handleExportDistributionCSV}
        containerRef={distributionRef}
      />
      <WeekdayPatternSection
        weekdayAverages={extended?.weekday_averages}
        loading={extendedLoading}
        error={extendedError}
      />
      <CorrelationSection
        title={t('stats.tagImpact')}
        rows={tagCorrelationRows}
        withoutLabel={t('stats.without')}
        emptyLabel={t('stats.tagImpactEmpty')}
        note={t('stats.tagImpactNote')}
        loading={extendedLoading}
        error={extendedError}
      />
      <CorrelationSection
        title={t('stats.goalImpact')}
        rows={goalCorrelationRows}
        withLabel={t('stats.completed')}
        withoutLabel={t('stats.without')}
        emptyLabel={t('stats.goalImpactEmpty')}
        note={t('stats.goalImpactNote')}
        loading={extendedLoading}
        error={extendedError}
      />
      <TagCorrelationsSection tagStats={tagStats} onExportCSV={handleExportTagCSV} />
      <HeatmapSection
        heatmap={heatmap}
        year={heatmapYear}
        onYearChange={setHeatmapYear}
        loading={heatmapLoading}
        error={heatmapError}
      />
      <MoodCalendarSection days={calendarDays} />
    </div>
  );
};

export default StatisticsView;
