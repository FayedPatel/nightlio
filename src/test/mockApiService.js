import { vi } from 'vitest';

// Full mock of the src/services/api default export, resolving to the shapes
// the hooks and contexts expect. Use with:
//   vi.mock('../services/api', async () => {
//     const { createMockApiService } = await import('../test/mockApiService');
//     return { default: createMockApiService() };
//   });
export const createMockApiService = (overrides = {}) => ({
  setAuthToken: vi.fn(),
  getPublicConfig: vi.fn().mockResolvedValue({
    enable_oidc: false,
    enable_mood_music: false,
    signup_url: null,
  }),
  verifyToken: vi.fn().mockResolvedValue({ user: { id: 1, name: 'Test User' } }),
  localLogin: vi
    .fn()
    .mockResolvedValue({ token: 'test-token', user: { id: 1, name: 'Test User' } }),
  logout: vi.fn().mockResolvedValue({}),
  getOidcLoginUrl: vi.fn().mockReturnValue('/api/auth/login/oidc'),
  getMoodEntries: vi.fn().mockResolvedValue([]),
  getEntrySelections: vi.fn().mockResolvedValue([]),
  createMoodEntry: vi.fn().mockResolvedValue({ entry_id: 101, new_achievements: [] }),
  updateMoodEntry: vi.fn().mockResolvedValue({ status: 'success', entry: null }),
  deleteMoodEntry: vi.fn().mockResolvedValue({}),
  getGroups: vi.fn().mockResolvedValue([]),
  createGroup: vi.fn().mockResolvedValue({}),
  createGroupOption: vi.fn().mockResolvedValue({}),
  deleteGroup: vi.fn().mockResolvedValue({}),
  getStatistics: vi.fn().mockResolvedValue({}),
  getCurrentStreak: vi.fn().mockResolvedValue({ current_streak: 0 }),
  getUserAchievements: vi.fn().mockResolvedValue([]),
  getAchievementsProgress: vi.fn().mockResolvedValue({}),
  getActivity: vi.fn().mockResolvedValue([]),
  getPreferences: vi.fn().mockResolvedValue({ theme: null }),
  updateThemePreference: vi.fn().mockResolvedValue({ status: 'success' }),
  getGoals: vi.fn().mockResolvedValue([]),
  createGoal: vi.fn().mockResolvedValue({}),
  deleteGoal: vi.fn().mockResolvedValue({}),
  getGoalCompletions: vi.fn().mockResolvedValue([]),
  incrementGoalProgress: vi.fn().mockResolvedValue({}),
  getMoodMusic: vi.fn().mockResolvedValue({}),
  exportPdf: vi.fn().mockResolvedValue(new Blob()),
  request: vi.fn().mockResolvedValue({}),
  ...overrides,
});
