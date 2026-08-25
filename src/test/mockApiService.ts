import { vi } from 'vitest';
import type { Mock } from 'vitest';
import type apiService from '../services/api';
import type { User } from '../types/api';

type ApiService = typeof apiService;

/**
 * The real ApiService surface with every method replaced by a vitest Mock
 * that preserves the method's exact signature (so mockResolvedValue payloads
 * are checked against the src/types/api.ts contract). Non-method members
 * (token) keep their real types.
 */
export type MockApiService = {
  [K in keyof ApiService]: ApiService[K] extends (...args: infer A) => infer R
    ? Mock<(...args: A) => R>
    : ApiService[K];
};

const testUser: User = { id: 1, name: 'Test User', email: null, avatar_url: null };

// Full mock of the src/services/api default export, resolving to the shapes
// the hooks and contexts expect. Use with:
//   vi.mock('../services/api', async () => {
//     const { createMockApiService } = await import('../test/mockApiService');
//     return { default: createMockApiService() };
//   });
export const createMockApiService = (overrides: Partial<MockApiService> = {}): MockApiService => ({
  token: null,
  setAuthToken: vi.fn<ApiService['setAuthToken']>(),
  buildUrl: vi.fn<ApiService['buildUrl']>((endpoint) =>
    endpoint.startsWith('/') ? endpoint : `/${endpoint}`,
  ),
  getPublicConfig: vi.fn<ApiService['getPublicConfig']>().mockResolvedValue({
    enable_oidc: false,
    enable_mood_music: false,
    enable_local_login: true,
    signup_url: null,
    version: '0.0.0-test',
  }),
  verifyToken: vi.fn<ApiService['verifyToken']>().mockResolvedValue({ user: testUser }),
  localLogin: vi
    .fn<ApiService['localLogin']>()
    .mockResolvedValue({ token: 'test-token', user: testUser }),
  register: vi
    .fn<ApiService['register']>()
    .mockResolvedValue({ status: 'success', user: testUser }),
  logout: vi.fn<ApiService['logout']>().mockResolvedValue({ status: 'success' }),
  getOidcLoginUrl: vi.fn<ApiService['getOidcLoginUrl']>().mockReturnValue('/api/auth/login/oidc'),
  getMoodEntries: vi.fn<ApiService['getMoodEntries']>().mockResolvedValue([]),
  getEntrySelections: vi.fn<ApiService['getEntrySelections']>().mockResolvedValue([]),
  createMoodEntry: vi.fn<ApiService['createMoodEntry']>().mockResolvedValue({
    status: 'success',
    entry_id: 101,
    new_achievements: [],
    message: 'ok',
  }),
  updateMoodEntry: vi.fn<ApiService['updateMoodEntry']>().mockResolvedValue({
    status: 'success',
    message: 'ok',
    entry: {
      id: 1,
      date: '2026-01-01',
      mood: 3,
      content: '',
      created_at: '2026-01-01 00:00:00',
      updated_at: '2026-01-01 00:00:00',
      selections: [],
    },
  }),
  deleteMoodEntry: vi
    .fn<ApiService['deleteMoodEntry']>()
    .mockResolvedValue({ status: 'success', message: 'ok' }),
  getGroups: vi.fn<ApiService['getGroups']>().mockResolvedValue([]),
  createGroup: vi
    .fn<ApiService['createGroup']>()
    .mockResolvedValue({ status: 'success', group_id: 1, message: 'ok' }),
  createGroupOption: vi
    .fn<ApiService['createGroupOption']>()
    .mockResolvedValue({ status: 'success', option_id: 1, message: 'ok' }),
  deleteGroup: vi
    .fn<ApiService['deleteGroup']>()
    .mockResolvedValue({ status: 'success', message: 'ok' }),
  getStatistics: vi.fn<ApiService['getStatistics']>().mockResolvedValue({
    statistics: {
      total_entries: 0,
      average_mood: 0,
      lowest_mood: null,
      highest_mood: null,
      first_entry_date: null,
      last_entry_date: null,
    },
    mood_distribution: {},
    current_streak: 0,
  }),
  getCurrentStreak: vi
    .fn<ApiService['getCurrentStreak']>()
    .mockResolvedValue({ current_streak: 0, message: 'Current streak: 0 days' }),
  recordStatisticsView: vi
    .fn<ApiService['recordStatisticsView']>()
    .mockResolvedValue({ counted: true }),
  getUserAchievements: vi.fn<ApiService['getUserAchievements']>().mockResolvedValue([]),
  checkAchievements: vi
    .fn<ApiService['checkAchievements']>()
    .mockResolvedValue({ new_achievements: [], count: 0 }),
  getAchievementsProgress: vi.fn<ApiService['getAchievementsProgress']>().mockResolvedValue({
    first_entry: { current: 0, max: 1 },
    week_warrior: { current: 0, max: 7 },
    consistency_king: { current: 0, max: 30 },
    data_lover: { current: 0, max: 10 },
    mood_master: { current: 0, max: 100 },
  }),
  // ActivityPage shape — the endpoint is paginated and never returns a bare
  // array (the old `[]` here was a wrong mock shape).
  getActivity: vi
    .fn<ApiService['getActivity']>()
    .mockResolvedValue({ activities: [], next_cursor: null }),
  getPreferences: vi.fn<ApiService['getPreferences']>().mockResolvedValue({ theme: null }),
  updateThemePreference: vi
    .fn<ApiService['updateThemePreference']>()
    .mockImplementation(async (theme) => ({ status: 'success', theme })),
  getGoals: vi.fn<ApiService['getGoals']>().mockResolvedValue([]),
  createGoal: vi.fn<ApiService['createGoal']>().mockResolvedValue({ id: 1 }),
  updateGoal: vi.fn<ApiService['updateGoal']>().mockResolvedValue({ status: 'ok' }),
  deleteGoal: vi.fn<ApiService['deleteGoal']>().mockResolvedValue({ status: 'ok' }),
  getGoalCompletions: vi.fn<ApiService['getGoalCompletions']>().mockResolvedValue([]),
  incrementGoalProgress: vi.fn<ApiService['incrementGoalProgress']>().mockResolvedValue({
    id: 1,
    user_id: 1,
    title: 'Goal',
    description: null,
    frequency_per_week: 1,
    completed: 1,
    streak: 0,
    period_start: '2026-01-05',
    last_completed_date: '2026-01-05',
    created_at: '2026-01-01 00:00:00',
    updated_at: '2026-01-05 00:00:00',
    already_completed_today: true,
    already_logged: false,
    logged_date: '2026-01-05',
  }),
  // Empty strings stay falsy so components that gate on audio_url behave the
  // same as with the old `{}` stub.
  getMoodMusic: vi
    .fn<ApiService['getMoodMusic']>()
    .mockResolvedValue({ audio_url: '', track_name: '', artist: '' }),
  // Empty by default: the picker degrades to bundled-English-only, matching
  // how a fresh/offline server (or the pre-IC current state) behaves.
  getLanguages: vi.fn<ApiService['getLanguages']>().mockResolvedValue({ languages: [] }),
  getLanguagePack: vi.fn<ApiService['getLanguagePack']>().mockResolvedValue({
    schema_version: 1,
    language: 'en',
    name: 'English',
    native_name: 'English',
    version: '0.0.0-test',
    strings: {},
  }),
  exportPdf: vi.fn<ApiService['exportPdf']>().mockResolvedValue(new Blob()),
  // Minimal valid v1 envelope / zero-count result — specs that care about
  // real payloads override these per-test.
  exportData: vi.fn<ApiService['exportData']>().mockResolvedValue({
    schema_version: 1,
    exported_at: '2026-01-01 00:00:00',
    app_version: '0.0.0-test',
    data: { entries: [], goals: [] },
  }),
  importData: vi.fn<ApiService['importData']>().mockResolvedValue({
    status: 'success',
    entries: { imported: 0, skipped: 0 },
    goals: { imported: 0, skipped: 0 },
  }),
  request: vi.fn<ApiService['request']>().mockResolvedValue({}),
  ...overrides,
});
