// Prefer Vite envs; allow overriding API base via VITE_API_URL in any mode
function normalizeBaseUrl(raw) {
  let v = raw ?? '';
  if (typeof v !== 'string') v = String(v);
  v = v.trim();
  // Handle cases like '""' or "''" injected by build-time env
  if (v === '""' || v === "''") v = '';
  // Strip surrounding quotes and any stray quotes
  v = v.replace(/^['"]+|['"]+$/g, '');
  v = v.replace(/["']/g, '');
  // Remove trailing slashes
  v = v.replace(/\/+$/g, '');
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

class ApiService {
  constructor() {
    this.token = null;
  }

  setAuthToken(token) {
    this.token = token;
  }

  buildUrl(endpoint) {
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

  async request(endpoint, options = {}) {
    const url = this.buildUrl(endpoint);
    const method = (options.method || 'GET').toUpperCase();
    const isMutation = !SAFE_METHODS.has(method);
    // NOTE: `headers` is intentionally computed AFTER `...options` so it is
    // the merge that wins. Spreading `...options` after `headers` would let
    // a raw `options.headers` (if the caller passed one) silently replace
    // this merged object outright, dropping Content-Type/credentials-related
    // headers set here.
    const config = {
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
          const errorData = await response.json().catch(() => ({}));
          if (errorData && (errorData.error || errorData.message)) {
            errorMessage = errorData.error || errorData.message;
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
      return await response.json();
    } catch (error) {
      console.error('API request failed:', error);
      throw error;
    }
  }

  // Public config
  async getPublicConfig() {
    return this.request('/api/config');
  }

  // Authentication endpoints
  async localLogin(username, password) {
    // With credentials: password login. Without: credential-free
    // single-user self-host login (the backend rejects it when OIDC is
    // configured).
    const options = { method: 'POST' };
    if (username != null || password != null) {
      options.body = JSON.stringify({ username, password });
    }
    return this.request('/api/auth/local/login', options);
  }

  async register({ username, password, email, name } = {}) {
    return this.request('/api/auth/local/register', {
      method: 'POST',
      body: JSON.stringify({ username, password, email, name }),
    });
  }

  // URL for the OIDC single sign-on redirect flow (top-level navigation,
  // not an XHR).
  getOidcLoginUrl() {
    return this.buildUrl('/api/auth/login/oidc');
  }

  // token is optional: when omitted, auth relies solely on the httpOnly
  // session cookie sent via credentials: 'include' (see request()) -- used
  // to restore a session that only exists as a cookie, e.g. after
  // localStorage was cleared.
  async verifyToken(token) {
    const options = { method: 'POST' };
    if (token) {
      options.headers = { Authorization: `Bearer ${token}` };
    }
    return this.request('/api/auth/verify', options);
  }

  // Clears the httpOnly session cookie server-side. Always safe to call,
  // even with no active session (idempotent) or an already-expired token.
  async logout() {
    return this.request('/api/auth/logout', { method: 'POST' });
  }

  // Mood entries endpoints
  async getMoodEntries() {
    return this.request('/api/moods');
  }

  async createMoodEntry(entryData) {
    return this.request('/api/mood', {
      method: 'POST',
      body: JSON.stringify(entryData),
    });
  }

  async updateMoodEntry(entryId, entryData) {
    return this.request(`/api/mood/${entryId}`, {
      method: 'PUT',
      body: JSON.stringify(entryData),
    });
  }

  async deleteMoodEntry(entryId) {
    return this.request(`/api/mood/${entryId}`, {
      method: 'DELETE',
    });
  }

  // Statistics endpoints
  async getStatistics() {
    return this.request('/api/statistics');
  }

  // Streak endpoint
  async getCurrentStreak() {
    return this.request('/api/streak');
  }

  // Activity feed endpoint (keyset-paginated: pass the previous page's
  // next_cursor as `before` to fetch older events)
  async getActivity(before, limit) {
    const params = new URLSearchParams();
    if (before != null) params.set('before', String(before));
    if (limit != null) params.set('limit', String(limit));
    const q = params.toString();
    return this.request(`/api/activity${q ? `?${q}` : ''}`);
  }

  // Mood music endpoint
  async getMoodMusic(tag) {
    const encodedTag = encodeURIComponent(String(tag || 'chill'));
    return this.request(`/api/music/vibe?tag=${encodedTag}`);
  }

  // Groups endpoints
  async getGroups() {
    return this.request('/api/groups');
  }

  async createGroup(groupData) {
    return this.request('/api/groups', {
      method: 'POST',
      body: JSON.stringify(groupData),
    });
  }

  async createGroupOption(groupId, optionData) {
    return this.request(`/api/groups/${groupId}/options`, {
      method: 'POST',
      body: JSON.stringify(optionData),
    });
  }

  async deleteGroup(groupId) {
    return this.request(`/api/groups/${groupId}`, {
      method: 'DELETE',
    });
  }

  // Entry selections endpoint
  async getEntrySelections(entryId) {
    return this.request(`/api/mood/${entryId}/selections`);
  }

  // Achievement endpoints
  async getUserAchievements() {
    return this.request('/api/achievements');
  }

  async checkAchievements() {
    return this.request('/api/achievements/check', {
      method: 'POST',
    });
  }

  async getAchievementsProgress() {
    return this.request('/api/achievements/progress');
  }

  // Web3 minting removed

  // -------- Goals endpoints --------
  async getGoals() {
    return this.request('/api/goals');
  }

  async createGoal(goal) {
    // Accept { title, description, frequency_per_week } or { title, description, frequency }
    const payload = { ...goal };
    if (payload.frequency && !payload.frequency_per_week) {
      // frequency like '3 days a week' -> 3
      const n = parseInt(String(payload.frequency).trim(), 10);
      if (!Number.isNaN(n)) payload.frequency_per_week = n;
      delete payload.frequency;
    }
    return this.request('/api/goals', {
      method: 'POST',
      body: JSON.stringify(payload),
    });
  }

  async updateGoal(goalId, patch) {
    const payload = { ...patch };
    if (payload.frequency && !payload.frequency_per_week) {
      const n = parseInt(String(payload.frequency).trim(), 10);
      if (!Number.isNaN(n)) payload.frequency_per_week = n;
      delete payload.frequency;
    }
    return this.request(`/api/goals/${goalId}`, {
      method: 'PUT',
      body: JSON.stringify(payload),
    });
  }

  async deleteGoal(goalId) {
    return this.request(`/api/goals/${goalId}`, { method: 'DELETE' });
  }

  async incrementGoalProgress(goalId) {
    return this.request(`/api/goals/${goalId}/progress`, { method: 'POST' });
  }

  async getGoalCompletions(goalId, { start, end } = {}) {
    const params = new URLSearchParams();
    if (start) params.set('start', start);
    if (end) params.set('end', end);
    const q = params.toString();
    return this.request(`/api/goals/${goalId}/completions${q ? `?${q}` : ''}`);
  }

  // Export endpoint
  async exportPdf(content) {
    const url = this.buildUrl('/api/export/pdf');

    const config = {
      method: 'POST',
      credentials: 'include',
      headers: {
        'Content-Type': 'application/json',
        [CSRF_HEADER_NAME]: CSRF_HEADER_VALUE,
      },
      body: JSON.stringify({ content }),
    };

    if (this.token) {
      config.headers.Authorization = `Bearer ${this.token}`;
    }

    const response = await fetch(url, config);
    if (!response.ok) {
      throw new Error(`Export failed: ${response.status}`);
    }
    return response.blob();
  }
}

const apiService = new ApiService();
export default apiService;