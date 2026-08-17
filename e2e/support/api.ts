import { request } from '@playwright/test';
import type { APIRequestContext } from '@playwright/test';
import type {
  CreateGoalResponse,
  CreateMoodEntryResponse,
  Goal,
  GoalCompletion,
  LoginResponse,
  MoodEntry,
  MoodValue,
} from '../../src/types/api';

// Each Playwright worker runs its own API instance; the worker fixture in
// fixtures.ts exports its port via E2E_API_PORT (workers are separate
// processes, so this is naturally per-worker). Read lazily — the fixture
// sets it after this module is imported.
const apiBase = () => `http://localhost:${process.env.E2E_API_PORT || 5000}`;

// Bearer-token API context against the e2e backend (credential-free local
// login; the e2e API always runs with OIDC disabled). The token is cached
// per worker — /api/auth/local/login is rate-limited to 30/min and
// every browser page load performs its own auto-login on top of these.
let cachedToken: string | null = null;

export interface ApiClient {
  ctx: APIRequestContext;
  headers: { Authorization: string };
}

export const apiContext = async (): Promise<ApiClient> => {
  const ctx = await request.newContext({ baseURL: apiBase() });
  if (!cachedToken) {
    const login = await ctx.post('/api/auth/local/login');
    if (!login.ok()) {
      throw new Error(`local login failed: ${login.status()} ${await login.text()}`);
    }
    const body: LoginResponse = await login.json();
    cachedToken = body.token;
  }
  return { ctx, headers: { Authorization: `Bearer ${cachedToken}` } };
};

// Delete every mood entry so each spec starts from a clean slate (the DB is
// shared across the run; workers are capped at 1 in playwright.config.ts).
export const wipeEntries = async (): Promise<void> => {
  const { ctx, headers } = await apiContext();
  const resp = await ctx.get('/api/moods', { headers });
  const entries: MoodEntry[] = await resp.json();
  for (const entry of entries) {
    await ctx.delete(`/api/mood/${entry.id}`, { headers });
  }
  await ctx.dispose();
};

export interface SeedEntryParams {
  date: string;
  mood?: MoodValue;
  content?: string;
}

export const seedEntry = async ({
  date,
  mood = 3,
  content = 'Seeded entry.',
}: SeedEntryParams): Promise<CreateMoodEntryResponse> => {
  const { ctx, headers } = await apiContext();
  const resp = await ctx.post('/api/mood', {
    headers,
    data: { mood, date, content, selected_options: [] },
  });
  if (resp.status() !== 201) {
    throw new Error(`seed failed: ${resp.status()} ${await resp.text()}`);
  }
  const body: CreateMoodEntryResponse = await resp.json();
  await ctx.dispose();
  return body;
};

export const listEntries = async (): Promise<MoodEntry[]> => {
  const { ctx, headers } = await apiContext();
  const resp = await ctx.get('/api/moods', { headers });
  const entries: MoodEntry[] = await resp.json();
  await ctx.dispose();
  return entries;
};

export const listGoals = async (): Promise<Goal[]> => {
  const { ctx, headers } = await apiContext();
  const resp = await ctx.get('/api/goals', { headers });
  const goals: Goal[] = await resp.json();
  await ctx.dispose();
  return goals;
};

export const listGoalCompletions = async (
  goalId: number,
): Promise<GoalCompletion[]> => {
  const { ctx, headers } = await apiContext();
  const resp = await ctx.get(`/api/goals/${goalId}/completions`, { headers });
  const rows: GoalCompletion[] = await resp.json();
  await ctx.dispose();
  return rows;
};

export interface SeedGoalParams {
  title: string;
  description?: string;
  frequency?: number;
}

export const seedGoal = async ({
  title,
  description = 'Seeded goal.',
  frequency = 3,
}: SeedGoalParams): Promise<CreateGoalResponse> => {
  const { ctx, headers } = await apiContext();
  const resp = await ctx.post('/api/goals', {
    headers,
    data: { title, description, frequency_per_week: frequency },
  });
  const body: CreateGoalResponse = await resp.json();
  await ctx.dispose();
  return body;
};

export const wipeGoals = async (): Promise<void> => {
  const { ctx, headers } = await apiContext();
  const resp = await ctx.get('/api/goals', { headers });
  const goals: Goal[] = await resp.json();
  for (const goal of goals) {
    await ctx.delete(`/api/goals/${goal.id}`, { headers });
  }
  await ctx.dispose();
};

export const isoDaysAgo = (days: number): string => {
  const date = new Date();
  date.setDate(date.getDate() - days);
  const pad = (v: number) => String(v).padStart(2, '0');
  return `${date.getFullYear()}-${pad(date.getMonth() + 1)}-${pad(date.getDate())}`;
};
