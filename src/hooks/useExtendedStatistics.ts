import { useState, useEffect, useCallback } from 'react';
import statsApi from '../services/statsApi';
import { translate } from '../i18n';
import type { ExtendedStats, Heatmap } from '../types/api';

const CURRENT_YEAR = new Date().getFullYear();

// Loads the Phase 3 extended statistics: one fetch for the aggregate bundle
// (rolling averages, weekday pattern, volatility, correlations, monthly
// digest) and one, keyed on the selected year, for the calendar heatmap.
// Each fetch keeps its own loading/error state so a failing section renders
// an inline error instead of blanking the whole statistics view.
export const useExtendedStatistics = () => {
  const [extended, setExtended] = useState<ExtendedStats | null>(null);
  const [extendedLoading, setExtendedLoading] = useState(true);
  const [extendedError, setExtendedError] = useState<string | null>(null);

  const [heatmap, setHeatmap] = useState<Heatmap | null>(null);
  const [heatmapLoading, setHeatmapLoading] = useState(true);
  const [heatmapError, setHeatmapError] = useState<string | null>(null);
  const [heatmapYear, setHeatmapYear] = useState(CURRENT_YEAR);

  const loadExtended = useCallback(async () => {
    setExtendedLoading(true);
    setExtendedError(null);
    try {
      const data = await statsApi.getExtendedStatistics();
      setExtended(data);
    } catch (error) {
      console.error('Failed to load extended statistics:', error);
      setExtendedError(translate('errors.loadExtendedStatistics'));
    } finally {
      setExtendedLoading(false);
    }
  }, []);

  const loadHeatmap = useCallback(async (year: number) => {
    setHeatmapLoading(true);
    setHeatmapError(null);
    try {
      const data = await statsApi.getHeatmap(year);
      setHeatmap(data);
    } catch (error) {
      console.error('Failed to load mood heatmap:', error);
      setHeatmapError(translate('errors.loadHeatmap'));
    } finally {
      setHeatmapLoading(false);
    }
  }, []);

  useEffect(() => {
    loadExtended();
  }, [loadExtended]);

  useEffect(() => {
    loadHeatmap(heatmapYear);
  }, [loadHeatmap, heatmapYear]);

  return {
    extended,
    extendedLoading,
    extendedError,
    heatmap,
    heatmapLoading,
    heatmapError,
    heatmapYear,
    setHeatmapYear,
  };
};

export default useExtendedStatistics;
