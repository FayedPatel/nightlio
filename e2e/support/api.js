import { request } from '@playwright/test';

const API_BASE = 'http://localhost:5000';

// Bearer-token API context against the e2e backend (credential-free local
// login; the e2e API always runs with OIDC disabled). The token is cached
// for the whole run — /api/auth/local/login is rate-limited to 30/min and
// every browser page load performs its own auto-login on top of these.
let cachedToken = null;

export const apiContext = async () => {
  const ctx = await request.newContext({ baseURL: API_BASE });
  if (!cachedToken) {
    const login = await ctx.post('/api/auth/local/login');
    if (!login.ok()) {
      throw new Error(`local login failed: ${login.status()}`);
    }
    ({ token: cachedToken } = await login.json());
  }
  return { ctx, headers: { Authorization: `Bearer ${cachedToken}` } };
};

// Delete every mood entry so each spec starts from a clean slate (the DB is
// shared across the run; workers are capped at 1 in playwright.config.js).
export const wipeEntries = async () => {
  const { ctx, headers } = await apiContext();
  const resp = await ctx.get('/api/moods', { headers });
  const entries = await resp.json();
  for (const entry of entries) {
    await ctx.delete(`/api/mood/${entry.id}`, { headers });
  }
  await ctx.dispose();
};

export const seedEntry = async ({ date, mood = 3, content = 'Seeded entry.' }) => {
  const { ctx, headers } = await apiContext();
  const resp = await ctx.post('/api/mood', {
    headers,
    data: { mood, date, content, selected_options: [] },
  });
  if (resp.status() !== 201) {
    throw new Error(`seed failed: ${resp.status()} ${await resp.text()}`);
  }
  const body = await resp.json();
  await ctx.dispose();
  return body;
};

export const listEntries = async () => {
  const { ctx, headers } = await apiContext();
  const resp = await ctx.get('/api/moods', { headers });
  const entries = await resp.json();
  await ctx.dispose();
  return entries;
};

export const listGoals = async () => {
  const { ctx, headers } = await apiContext();
  const resp = await ctx.get('/api/goals', { headers });
  const goals = await resp.json();
  await ctx.dispose();
  return goals;
};

export const seedGoal = async ({ title, description = 'Seeded goal.', frequency = 3 }) => {
  const { ctx, headers } = await apiContext();
  const resp = await ctx.post('/api/goals', {
    headers,
    data: { title, description, frequency_per_week: frequency },
  });
  const body = await resp.json();
  await ctx.dispose();
  return body;
};

export const wipeGoals = async () => {
  const { ctx, headers } = await apiContext();
  const resp = await ctx.get('/api/goals', { headers });
  const goals = await resp.json();
  for (const goal of goals) {
    await ctx.delete(`/api/goals/${goal.id}`, { headers });
  }
  await ctx.dispose();
};

export const isoDaysAgo = (days) => {
  const date = new Date();
  date.setDate(date.getDate() - days);
  const pad = (v) => String(v).padStart(2, '0');
  return `${date.getFullYear()}-${pad(date.getMonth() + 1)}-${pad(date.getDate())}`;
};
