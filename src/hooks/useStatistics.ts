import { useState, useEffect, useCallback } from 'react';
import apiService from '../services/api';
import type { Statistics } from '../types/api';

export const useStatistics = () => {
  const [statistics, setStatistics] = useState<Statistics | null>(null);
  const [currentStreak, setCurrentStreak] = useState(0);
  const [loading, setLoading] = useState(false);
  const [error, setError] = useState<string | null>(null);

  const loadStatistics = useCallback(async () => {
    setLoading(true);
    setError(null);
    
    try {
      const data = await apiService.getStatistics();
      setStatistics(data);
    } catch (error) {
      console.error('Failed to load statistics:', error);
      setError('Failed to load statistics');
    } finally {
      setLoading(false);
    }
  }, []);

  const loadStreak = useCallback(async () => {
    try {
      const data = await apiService.getCurrentStreak();
      setCurrentStreak(data.current_streak);
    } catch (error) {
      console.error('Failed to load streak:', error);
      setCurrentStreak(0);
    }
  }, []);

  useEffect(() => {
    loadStreak();
  }, [loadStreak]);

  return {
    statistics,
    currentStreak,
    loading,
    error,
    loadStatistics,
    refreshStreak: loadStreak,
  };
};