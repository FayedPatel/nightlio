import { useMemo } from 'react';
import { getWeeklyMoodData } from '../../utils/moodUtils';
import {
  DEFAULT_METRICS,
  EMPTY_OBJECT,
  buildCalendarDays,
  buildMoodDistributionData,
  aggregateTagStats,
  buildOverviewCards,
} from './statisticsViewUtils';
import { buildRollingOverlay } from './extendedStatsUtils';

// rollingSeries is the server-computed rolling_averages.series from
// /api/statistics/extended; null while loading or on error, in which case
// the trend chart simply renders without the overlay lines.
const useStatisticsViewData = (statistics, pastEntries, range, rollingSeries = null) => {
  const hasStatistics = Boolean(statistics);
  const metrics = statistics?.statistics ?? DEFAULT_METRICS;
  const currentStreak = statistics?.current_streak ?? 0;

  const moodDistribution = useMemo(
    () => statistics?.mood_distribution ?? EMPTY_OBJECT,
    [statistics?.mood_distribution],
  );

  const weeklyMoodData = useMemo(() => getWeeklyMoodData(pastEntries, range), [pastEntries, range]);

  const rollingOverlay = useMemo(
    () => buildRollingOverlay(rollingSeries, range),
    [rollingSeries, range],
  );

  const trendChartData = useMemo(
    () => weeklyMoodData.map((point, index) => ({ ...point, ...rollingOverlay[index] })),
    [weeklyMoodData, rollingOverlay],
  );

  const moodDistributionData = useMemo(
    () => buildMoodDistributionData(moodDistribution),
    [moodDistribution],
  );

  const tagStats = useMemo(() => aggregateTagStats(pastEntries), [pastEntries]);

  const calendarDays = useMemo(() => buildCalendarDays(pastEntries), [pastEntries]);

  const bestDayCount = useMemo(() => {
    const counts = Object.values(moodDistribution ?? {});
    return counts.length ? Math.max(...counts) : 0;
  }, [moodDistribution]);

  const overviewCards = useMemo(
    () =>
      buildOverviewCards({
        totalEntries: metrics.total_entries ?? 0,
        averageMood: metrics.average_mood,
        currentStreak,
        bestDayCount,
      }),
    [metrics.total_entries, metrics.average_mood, currentStreak, bestDayCount],
  );

  return {
    hasStatistics,
    weeklyMoodData,
    trendChartData,
    moodDistribution,
    moodDistributionData,
    tagStats,
    calendarDays,
    overviewCards,
  };
};

export default useStatisticsViewData;
