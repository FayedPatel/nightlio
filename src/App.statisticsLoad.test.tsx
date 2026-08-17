// Regression tests for the statistics double-fetch: Sidebar and BottomNav
// (both always mounted) each used to fire onLoadStatistics on the stats
// route, so every visit fetched /api/statistics twice (four times under dev
// StrictMode) — and, before the contract change made that GET pure, double-counted the
// data_lover metric. The load now lives in a single AppContent effect that
// also records the view via POST /api/statistics/view.
import { describe, expect, it, vi, beforeEach } from 'vitest';
import { render, screen, fireEvent, waitFor } from '@testing-library/react';
import { MemoryRouter } from 'react-router-dom';
import App from './App';
import apiService from './services/api';
import type { MockApiService } from './test/mockApiService';

vi.mock('./services/api', async () => {
  const { createMockApiService } = await import('./test/mockApiService');
  return { default: createMockApiService() };
});
vi.mock('./components/mood/MusicDock', () => ({ default: () => null }));

const mockApi = apiService as unknown as MockApiService;

beforeEach(() => {
  localStorage.setItem('nightlio_token', 'test-token');
  mockApi.getStatistics.mockClear();
  mockApi.recordStatisticsView.mockClear();
});

describe('statistics route data loading', () => {
  it('fetches statistics exactly once on direct navigation to /dashboard/stats', async () => {
    render(
      <MemoryRouter initialEntries={['/dashboard/stats']}>
        <App />
      </MemoryRouter>,
    );

    await waitFor(() => expect(mockApi.getStatistics).toHaveBeenCalledTimes(1));
    await waitFor(() => expect(mockApi.recordStatisticsView).toHaveBeenCalledTimes(1));

    // Still exactly one of each after the initial render settles — with the
    // old Sidebar + BottomNav effects this was 2 fetches per visit.
    expect(mockApi.getStatistics).toHaveBeenCalledTimes(1);
    expect(mockApi.recordStatisticsView).toHaveBeenCalledTimes(1);
  });

  it('fetches statistics exactly once when entering the stats route from home', async () => {
    render(
      <MemoryRouter initialEntries={['/dashboard']}>
        <App />
      </MemoryRouter>,
    );

    // Not on the stats route yet: no statistics fetch.
    await screen.findByTitle('Statistics');
    expect(mockApi.getStatistics).not.toHaveBeenCalled();

    fireEvent.click(screen.getByTitle('Statistics'));

    await waitFor(() => expect(mockApi.getStatistics).toHaveBeenCalledTimes(1));
    expect(mockApi.getStatistics).toHaveBeenCalledTimes(1);
    expect(mockApi.recordStatisticsView).toHaveBeenCalledTimes(1);
  });

  it('still renders the stats view when recordStatisticsView fails', async () => {
    mockApi.recordStatisticsView.mockRejectedValue(new Error('metrics down'));

    render(
      <MemoryRouter initialEntries={['/dashboard/stats']}>
        <App />
      </MemoryRouter>,
    );

    // The statistics fetch proceeds regardless of the metrics failure.
    await waitFor(() => expect(mockApi.getStatistics).toHaveBeenCalledTimes(1));
    expect(mockApi.recordStatisticsView).toHaveBeenCalledTimes(1);
  });
});
