import type {
  Achievement,
  AchievementProgress,
  ActivityPage,
  AppConfig,
  CheckAchievementsResponse,
  CreateGoalRequest,
  CreateGoalResponse,
  CreateGroupOptionRequest,
  CreateGroupOptionResponse,
  CreateGroupRequest,
  CreateGroupResponse,
  CreateMoodEntryRequest,
  CreateMoodEntryResponse,
  DeleteResponse,
  EntrySelection,
  Goal,
  GoalCompletion,
  GoalProgressResponse,
  GoalStatusOkResponse,
  Group,
  LoginResponse,
  LogoutResponse,
  MoodEntry,
  MusicTrack,
  Preferences,
  RecordStatisticsViewResponse,
  RegisterRequest,
  RegisterResponse,
  Statistics,
  Streak,
  ThemeName,
  UpdateGoalRequest,
  UpdateMoodEntryRequest,
  UpdateMoodEntryResponse,
  UpdatePreferencesResponse,
  VerifyTokenResponse,
} from '../types/api';

// Prefer Vite envs; allow overriding API base via VITE_API_URL in any mode
function normalizeBaseUrl(raw: unknown): string {
  const rawValue = raw ?? '';
  let v = typeof rawValue === 'string' ? rawValue : String(rawValue);
  v = v.trim();
  // Handle cases like '""' or "''" injected by build-time env
  if (v === '""' || v === "''") v = '';
  // Strip surrounding quotes and any stray quotes
  v = v.replace(/^['"]+|['"]+$/g, '');
  v = v.replace(/["']/g, '');
  // Remove trailing slashes
  v = v.replace(/\/+$/g, '');
  // Reject bases with a non-http(s) scheme (file:, javascript:, ...): a
  // value like 'file:///x' would otherwise be treated as a path prefix and
  // resolve to an absolute file: URL in the browser. Fall back to the
  // relative /api mode instead.
  if (/^[a-z][a-z0-9+.-]*:/i.test(v) && !/^https?:\/\//i.test(v)) {
    console.warn(`Ignoring VITE_API_URL with unsupported scheme: ${v}`);
    return '';
  }
  return v;
}

const API_BASE_URL = normalizeBaseUrl(
  (typeof import.meta !== 'undefined' && import.meta.env && 'VITE_API_URL' in import.meta.env)
    ? import.meta.env.VITE_API_URL
    : '' // Use relative /api in both dev and prod; Vite proxy handles dev, nginx handles prod
);

// Sent on every mutating request. Its presence lets api/utils/auth_middleware.py
// tell a same-site fetch() apart from a cross-site HTML form submission when
// the request is cookie-authenticated: the header forces the browser to send
// a CORS preflight, and our CORS config (explicit CORS_ORIGINS, never '*')
// rejects that preflight from a foreign origin. Bearer-token requests don't
// need it (see backend docstring), but sending it unconditionally on
// mutations is simpler than tracking which auth mode is active client-side.
const CSRF_HEADER_NAME = 'X-Requested-With';
const CSRF_HEADER_VALUE = 'nightlio';
const SAFE_METHODS = new Set(['GET', 'HEAD']);

/**
 * fetch RequestInit with headers narrowed to a plain record so the merge in
 * request() (object spread) is well-typed — Headers instances and tuple
 * arrays would not spread correctly there.
 */
export interface ApiRequestOptions extends Omit<RequestInit, 'headers'> {
  headers?: Record<string, string>;
}

class ApiService {
  token: string | null;

  constructor() {
    this.token = null;
  }

  setAuthToken(token: string | null): void {
    this.token = token;
  }

  buildUrl(endpoint: string): string {
    // Safe-join base + endpoint, honoring relative mode when base is empty
    const path = endpoint.startsWith('/') ? endpoint : `/${endpoint}`;
    // If base is a relative prefix like '/api', and endpoint already starts with '/api',
    // avoid double-prefixing (i.e., '/api' + '/api/config' -> '/api/config').
    const base = API_BASE_URL;
    if (!base) {
      return path;
    }
    if (/^https?:\/\//i.test(base)) {
      return `${base}${path}`;
    }
    // Treat base as a path prefix
    const baseNoTrail = base.replace(/\/+$/g, '');
    if (path === baseNoTrail || path.startsWith(`${baseNoTrail}/`)) {
      return path; // endpoint already includes the base prefix
    }
    return `${baseNoTrail}${path}`;
  }

  async request<T>(endpoint: string, options: ApiRequestOptions = {}): Promise<T> {
    const url = this.buildUrl(endpoint);
    const method = (options.method || 'GET').toUpperCase();
    const isMutation = !SAFE_METHODS.has(method);
    // NOTE: `headers` is intentionally computed AFTER `...options` so it is
    // the merge that wins. Spreading `...options` after `headers` would let
    // a raw `options.headers` (if the caller passed one) silently replace
    // this merged object outright, dropping Content-Type/credentials-related
    // headers set here.
    const config: RequestInit & { headers: Record<string, string> } = {
      // Send the httpOnly session cookie (api/utils/auth_cookies.py) with
      // every request, same-origin or cross-origin (the latter needs the
      // backend's CORS supports_credentials=True, see api/app.py). Bearer
      // requests are unaffected -- the cookie is simply extra, redundant
      // credentials that the backend only consults when no Authorization
      // header is present.
      credentials: 'include',
      ...options,
      headers: {
        'Content-Type': 'application/json',
        ...(isMutation ? { [CSRF_HEADER_NAME]: CSRF_HEADER_VALUE } : {}),
        ...options.headers,
      },
    };

    if (this.token) {
      config.headers.Authorization = `Bearer ${this.token}`;
      // console.log('API Request with token:', this.token.substring(0, 20) + '...');
    } else {
      // console.log('API Request WITHOUT token');
    }

    try {
      const response = await fetch(url, config);
      if (!response.ok) {
        // Try to parse JSON error, else include text snippet to aid debugging
        let errorMessage = `HTTP error! status: ${response.status}`;
        const ct = response.headers.get('content-type') || '';
        if (ct.includes('application/json')) {
          // response.json() is Promise<any>; treat the untrusted body as a
          // loose Partial<ApiError>-plus-message record without widening types
          // elsewhere. Same truthiness semantics as the original JS.
          const errorData: Partial<Record<'error' | 'message', string>> =
            await response.json().catch(() => ({}));
          if (errorData && (errorData.error || errorData.message)) {
            errorMessage = errorData.error || errorData.message || errorMessage;
          }
        } else {
          const text = await response.text().catch(() => '');
          if (text) errorMessage += ` | body: ${text.slice(0, 200)}`;
        }
        throw new Error(errorMessage);
      }
      // Parse JSON safely
      const ct = response.headers.get('content-type') || '';
      if (!ct.includes('application/json')) {
        const text = await response.text();
        throw new Error(`Expected JSON but received: ${ct || 'unknown'} | body: ${text.slice(0, 200)}`);
      }
      // response.json() is Promise<any>; the per-method concrete return types
      // on the wrappers below are the trusted contract annotation.
      return await response.json();
    } catch (error) {
      console.error('API request failed:', error);
      throw error;
    }
  }

  // Public config
  async getPublicConfig(): Promise<AppConfig> {
    return this.request<AppConfig>('/api/config');
  }

  // Authentication endpoints
  async localLogin(username?: string, password?: string): Promise<LoginResponse> {
    // With credentials: password login. Without: credential-free
    // single-user self-host login (the backend rejects it when OIDC is
    // configured).
    const options: ApiRequestOptions = { method: 'POST' };
    if (username != null || password != null) {
      options.body = JSON.stringify({ username, password });
    }
    return this.request<LoginResponse>('/api/auth/local/login', options);
  }

  async register({ username, password, email, name }: Partial<RegisterRequest> = {}): Promise<RegisterResponse> {
    return this.request<RegisterResponse>('/api/auth/local/register', {
      method: 'POST',
      body: JSON.stringify({ username, password, email, name }),
    });
  }

  // URL for the OIDC single sign-on redirect flow (top-level navigation,
  // not an XHR).
  getOidcLoginUrl(): string {
    return this.buildUrl('/api/auth/login/oidc');
  }

  // token is optional: when omitted, auth relies solely on the httpOnly
  // session cookie sent via credentials: 'include' (see request()) -- used
  // to restore a session that only exists as a cookie, e.g. after
  // localStorage was cleared.
  async verifyToken(token?: string | null): Promise<VerifyTokenResponse> {
    const options: ApiRequestOptions = { method: 'POST' };
    if (token) {
      options.headers = { Authorization: `Bearer ${token}` };
    }
    return this.request<VerifyTokenResponse>('/api/auth/verify', options);
  }

  // Clears the httpOnly session cookie server-side. Always safe to call,
  // even with no active session (idempotent) or an already-expired token.
  async logout(): Promise<LogoutResponse> {
    return this.request<LogoutResponse>('/api/auth/logout', { method: 'POST' });
  }

  // Mood entries endpoints
  async getMoodEntries(): Promise<MoodEntry[]> {
    return this.request<MoodEntry[]>('/api/moods');
  }

  async createMoodEntry(entryData: CreateMoodEntryRequest): Promise<CreateMoodEntryResponse> {
    return this.request<CreateMoodEntryResponse>('/api/mood', {
      method: 'POST',
      body: JSON.stringify(entryData),
    });
  }

  async updateMoodEntry(entryId: number, entryData: UpdateMoodEntryRequest): Promise<UpdateMoodEntryResponse> {
    return this.request<UpdateMoodEntryResponse>(`/api/mood/${entryId}`, {
      method: 'PUT',
      body: JSON.stringify(entryData),
    });
  }

  async deleteMoodEntry(entryId: number): Promise<DeleteResponse> {
    return this.request<DeleteResponse>(`/api/mood/${entryId}`, {
      method: 'DELETE',
    });
  }

  // Statistics endpoints
  async getStatistics(): Promise<Statistics> {
    return this.request<Statistics>('/api/statistics');
  }

  // Records a statistics view for the data_lover achievement (at most one
  // counted view per calendar day; the GET above is a pure read as of the rewrite).
  async recordStatisticsView(): Promise<RecordStatisticsViewResponse> {
    return this.request<RecordStatisticsViewResponse>('/api/statistics/view', {
      method: 'POST',
    });
  }

  // Streak endpoint
  async getCurrentStreak(): Promise<Streak> {
    return this.request<Streak>('/api/streak');
  }

  async getPreferences(): Promise<Preferences> {
    return this.request<Preferences>('/api/preferences');
  }

  async updateThemePreference(theme: ThemeName): Promise<UpdatePreferencesResponse> {
    return this.request<UpdatePreferencesResponse>('/api/preferences', {
      method: 'PUT',
      body: JSON.stringify({ theme }),
    });
  }

  // Activity feed endpoint (keyset-paginated: pass the previous page's
  // next_cursor as `before` to fetch older events)
  async getActivity(before?: number | null, limit?: number | null): Promise<ActivityPage> {
    const params = new URLSearchParams();
    if (before != null) params.set('before', String(before));
    if (limit != null) params.set('limit', String(limit));
    const q = params.toString();
    return this.request<ActivityPage>(`/api/activity${q ? `?${q}` : ''}`);
  }

  // Mood music endpoint
  async getMoodMusic(tag?: string | null): Promise<MusicTrack> {
    const encodedTag = encodeURIComponent(String(tag || 'chill'));
    return this.request<MusicTrack>(`/api/music/vibe?tag=${encodedTag}`);
  }

  // Groups endpoints
  async getGroups(): Promise<Group[]> {
    return this.request<Group[]>('/api/groups');
  }

  async createGroup(groupData: CreateGroupRequest): Promise<CreateGroupResponse> {
    return this.request<CreateGroupResponse>('/api/groups', {
      method: 'POST',
      body: JSON.stringify(groupData),
    });
  }

  async createGroupOption(groupId: number, optionData: CreateGroupOptionRequest): Promise<CreateGroupOptionResponse> {
    return this.request<CreateGroupOptionResponse>(`/api/groups/${groupId}/options`, {
      method: 'POST',
      body: JSON.stringify(optionData),
    });
  }

  async deleteGroup(groupId: number): Promise<DeleteResponse> {
    return this.request<DeleteResponse>(`/api/groups/${groupId}`, {
      method: 'DELETE',
    });
  }

  // Entry selections endpoint
  async getEntrySelections(entryId: number): Promise<EntrySelection[]> {
    return this.request<EntrySelection[]>(`/api/mood/${entryId}/selections`);
  }

  // Achievement endpoints
  async getUserAchievements(): Promise<Achievement[]> {
    return this.request<Achievement[]>('/api/achievements');
  }

  async checkAchievements(): Promise<CheckAchievementsResponse> {
    return this.request<CheckAchievementsResponse>('/api/achievements/check', {
      method: 'POST',
    });
  }

  async getAchievementsProgress(): Promise<AchievementProgress> {
    return this.request<AchievementProgress>('/api/achievements/progress');
  }

  // Web3 minting removed

  // -------- Goals endpoints --------
  async getGoals(): Promise<Goal[]> {
    return this.request<Goal[]>('/api/goals');
  }

  async createGoal(goal: CreateGoalRequest): Promise<CreateGoalResponse> {
    // Accept { title, description, frequency_per_week } or { title, description, frequency }
    const payload = { ...goal };
    if (payload.frequency && !payload.frequency_per_week) {
      // frequency like '3 days a week' -> 3
      const n = parseInt(String(payload.frequency).trim(), 10);
      if (!Number.isNaN(n)) payload.frequency_per_week = n;
      delete payload.frequency;
    }
    return this.request<CreateGoalResponse>('/api/goals', {
      method: 'POST',
      body: JSON.stringify(payload),
    });
  }

  async updateGoal(goalId: number, patch: UpdateGoalRequest): Promise<GoalStatusOkResponse> {
    const payload = { ...patch };
    if (payload.frequency && !payload.frequency_per_week) {
      const n = parseInt(String(payload.frequency).trim(), 10);
      if (!Number.isNaN(n)) payload.frequency_per_week = n;
      delete payload.frequency;
    }
    return this.request<GoalStatusOkResponse>(`/api/goals/${goalId}`, {
      method: 'PUT',
      body: JSON.stringify(payload),
    });
  }

  async deleteGoal(goalId: number): Promise<GoalStatusOkResponse> {
    return this.request<GoalStatusOkResponse>(`/api/goals/${goalId}`, { method: 'DELETE' });
  }

  async incrementGoalProgress(goalId: number, date?: string | null): Promise<GoalProgressResponse> {
    return this.request<GoalProgressResponse>(`/api/goals/${goalId}/progress`, {
      method: 'POST',
      ...(date ? { body: JSON.stringify({ date }) } : {}),
    });
  }

  async getGoalCompletions(
    goalId: number,
    { start, end }: { start?: string; end?: string } = {},
  ): Promise<GoalCompletion[]> {
    const params = new URLSearchParams();
    if (start) params.set('start', start);
    if (end) params.set('end', end);
    const q = params.toString();
    return this.request<GoalCompletion[]>(`/api/goals/${goalId}/completions${q ? `?${q}` : ''}`);
  }

  // Export endpoint
  async exportPdf(content: string): Promise<Blob> {
    const url = this.buildUrl('/api/export/pdf');

    const headers: Record<string, string> = {
      'Content-Type': 'application/json',
      [CSRF_HEADER_NAME]: CSRF_HEADER_VALUE,
    };

    if (this.token) {
      headers.Authorization = `Bearer ${this.token}`;
    }

    const config: RequestInit = {
      method: 'POST',
      credentials: 'include',
      headers,
      body: JSON.stringify({ content }),
    };

    const response = await fetch(url, config);
    if (!response.ok) {
      throw new Error(`Export failed: ${response.status}`);
    }
    return response.blob();
  }
}

const apiService = new ApiService();
export default apiService;
