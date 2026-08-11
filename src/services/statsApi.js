// Extended statistics endpoints (Phase 3).
//
// Lives in its own module (rather than api.js) so it can land without
// touching src/services/api.js. It delegates every call to the shared
// apiService.request(), which owns the auth-header pattern: the Bearer
// token set via setAuthToken(), the httpOnly session cookie sent with
// credentials: 'include', and the shared error handling. Duplicating that
// logic here would fork the token state — AuthContext only ever calls
// apiService.setAuthToken().
import apiService from './api';

const statsApi = {
  // GET /api/statistics/extended
  // -> { rolling_averages, weekday_averages, mood_volatility,
  //      tag_correlations, goal_correlations, monthly_digest }
  getExtendedStatistics() {
    return apiService.request('/api/statistics/extended');
  },

  // GET /api/statistics/heatmap?year=YYYY
  // -> { year, days_logged, days: [{ date, average_mood, entry_count }] }
  // Only logged days are present; year defaults to the current year server-side.
  getHeatmap(year) {
    const params = new URLSearchParams();
    if (year != null) params.set('year', String(year));
    const q = params.toString();
    return apiService.request(`/api/statistics/heatmap${q ? `?${q}` : ''}`);
  },

  // GET /api/statistics/digest?year=YYYY&month=MM
  // -> { year, month, entries_logged, average_mood, previous_average_mood,
  //      mood_trend, top_tags, longest_streak }
  // Both params optional; server defaults to the current month.
  getMonthlyDigest(year, month) {
    const params = new URLSearchParams();
    if (year != null) params.set('year', String(year));
    if (month != null) params.set('month', String(month));
    const q = params.toString();
    return apiService.request(`/api/statistics/digest${q ? `?${q}` : ''}`);
  },
};

export default statsApi;
