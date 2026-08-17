// README capture utility, not a test: README_CAPTURES=1 yarn playwright test
// e2e/readme-captures.spec.ts --project=chromium
// Seeds a presentable account (a week of markdown entries, goals with
// progress, unlocked achievements), then saves the README's still shots to
// docs/assets/ and GIF keyframes to screenshots/readme-frames/ (assembled
// into GIFs by scripts/build-readme-gifs.mjs). Stills use the synthwave
// theme; long pages and interactive flows are captured as GIF frame
// sequences instead of tall stills. Skipped in normal runs/CI.
import { mkdirSync, writeFileSync } from 'node:fs';
import type { Page } from '@playwright/test';
import { test, expect } from './support/fixtures';
import {
  apiContext,
  wipeEntries,
  wipeGoals,
  seedEntry,
  seedGoal,
  isoDaysAgo,
} from './support/api';
import type { MoodValue } from '../src/types/api';

test.skip(!process.env.README_CAPTURES, 'set README_CAPTURES=1 to capture');

const STILL_DIR = 'docs/assets';
const FRAME_DIR = 'screenshots/readme-frames';
const DESKTOP = { width: 1360, height: 850 };
const GIF_VIEW = { width: 960, height: 600 };
const PHONE = { width: 412, height: 915 };

const ENTRIES: Array<{ day: number; mood: MoodValue; content: string }> = [
  { day: 0, mood: 5, content: '# Shipped the rewrite\nEverything green on the first run. Celebrated with a long walk and *way* too much coffee.' },
  { day: 1, mood: 4, content: '# Quiet focus day\nDeep work most of the morning, gym in the evening.\n\n- finished the migration notes\n- 5k on the treadmill' },
  { day: 2, mood: 3, content: '# Middling\nSlept badly, but the afternoon picked up after a proper lunch break.' },
  { day: 3, mood: 4, content: '# Board-game night\nLost twice at Wingspan, still worth it.' },
  { day: 4, mood: 2, content: '# Rough one\nServer alerts at 3am. Wrote down what to automate so it never pages me again.' },
  { day: 5, mood: 4, content: '# Recovery\nSlow morning, long reading session, early night.' },
  { day: 6, mood: 5, content: '# Hike day\nTwelve kilometres of forest trail and zero notifications.' },
];

const setTheme = async (theme: string) => {
  const { ctx, headers } = await apiContext();
  await ctx.put('/api/preferences', { headers, data: { theme } });
  await ctx.dispose();
};

// Real-motion capture via Chrome's CDP screencast: Chromium streams a frame
// on every visual change, so flows and scrolls come out smooth instead of a
// keyframe slideshow. Frames are written as NNNN-<ms offset>.png; the GIF
// assembler turns the timestamp gaps into per-frame delays.
const startScreencast = async (page: Page, dir: string) => {
  mkdirSync(dir, { recursive: true });
  const cdp = await page.context().newCDPSession(page);
  const frames: Array<{ data: string; ts: number }> = [];
  cdp.on('Page.screencastFrame', ev => {
    frames.push({ data: ev.data, ts: ev.metadata.timestamp ?? 0 });
    cdp.send('Page.screencastFrameAck', { sessionId: ev.sessionId }).catch(() => {});
  });
  await cdp.send('Page.startScreencast', {
    format: 'png',
    maxWidth: 880,
    maxHeight: 560,
    everyNthFrame: 1,
  });
  return async () => {
    await cdp.send('Page.stopScreencast').catch(() => {});
    await cdp.detach().catch(() => {});
    const t0 = frames[0]?.ts ?? 0;
    frames.forEach((f, i) => {
      const ms = Math.max(0, Math.round((f.ts - t0) * 1000));
      writeFileSync(`${dir}/${String(i).padStart(4, '0')}-${ms}.png`, Buffer.from(f.data, 'base64'));
    });
  };
};

test.beforeAll(async () => {
  await wipeEntries();
  await wipeGoals();
  for (const e of ENTRIES) {
    await seedEntry({ date: isoDaysAgo(e.day), mood: e.mood, content: e.content });
  }
  const gym = await seedGoal({ title: 'Train 3x a week', description: 'Any workout counts.', frequency: 3 });
  await seedGoal({ title: 'Read before bed', description: 'Twenty minutes, no phone.', frequency: 5 });
  const { ctx, headers } = await apiContext();
  await ctx.post(`/api/goals/${gym.goal_id}/progress`, { headers, data: {} });
  await ctx.post('/api/achievements/check', { headers, data: {} });
  await ctx.dispose();
  // README stills are captured in the synthwave theme.
  await setTheme('synthwave');
});

test('desktop stills (synthwave)', async ({ page }) => {
  test.setTimeout(180_000);
  await page.setViewportSize(DESKTOP);
  const shot = (name: string) => page.screenshot({ path: `${STILL_DIR}/${name}.png` });

  await page.goto('/');
  await expect(page.getByRole('heading', { name: /your moods/i })).toBeVisible();
  await shot('landing');

  await page.goto('/dashboard');
  await expect(page.locator('.mood-grid').first()).toBeVisible();
  await shot('dashboard');

  await page.goto('/dashboard/history');
  await expect(page.getByRole('button', { name: /Open entry from/ }).first()).toBeVisible();
  await shot('history');

  await page.goto('/dashboard/goals');
  await expect(page.getByText('Train 3x a week')).toBeVisible();
  await shot('goals');

  await page.goto('/dashboard/achievements');
  await expect(page.getByText(/Week Warrior/i).first()).toBeVisible();
  await shot('achievements');
});

test('phone still (synthwave)', async ({ page }) => {
  test.setTimeout(120_000);
  await page.setViewportSize(PHONE);
  await page.goto('/dashboard');
  await expect(page.locator('.mood-grid').first()).toBeVisible();
  await page.screenshot({ path: `${STILL_DIR}/mobile-dashboard.png` });
});

test('gif frames: statistics scroll', async ({ page }) => {
  // The stats page is the longest in the app — a smooth scroll-through GIF
  // shows the whole thing without a skyscraper still.
  test.setTimeout(180_000);
  await page.setViewportSize(GIF_VIEW);
  await page.goto('/dashboard/stats');
  await expect(page.locator('.recharts-surface').first()).toBeVisible({ timeout: 15_000 });
  await page.waitForTimeout(800);
  const stop = await startScreencast(page, `${FRAME_DIR}/stats-scroll`);
  await page.waitForTimeout(700);
  // Steady glide to the bottom: many small instant steps at screencast rate
  // read as continuous motion (CSS smooth-scroll would overshoot per step).
  const total = await page.evaluate(
    () => document.documentElement.scrollHeight - window.innerHeight,
  );
  const STEPS = 90;
  for (let i = 1; i <= STEPS; i += 1) {
    await page.evaluate(top => window.scrollTo(0, top), Math.round((total * i) / STEPS));
    await page.waitForTimeout(55);
  }
  await page.waitForTimeout(1000);
  await stop();
});

test('gif frames: log a mood', async ({ page }) => {
  test.setTimeout(180_000);
  await page.setViewportSize(GIF_VIEW);
  await page.goto('/dashboard');
  await expect(page.locator('.mood-grid').first()).toBeVisible();
  const stop = await startScreencast(page, `${FRAME_DIR}/log-mood`);
  await page.waitForTimeout(800);
  await page.locator('.mood-grid').first().getByTitle('Good').hover();
  await page.waitForTimeout(500);
  await page.locator('.mood-grid').first().getByTitle('Good').click();
  await expect(page).toHaveURL(/\/dashboard\/entry$/);
  const editor = page.locator('.mdx-editor [contenteditable]').first();
  await expect(editor).toBeVisible();
  await page.waitForTimeout(600);
  await editor.click();
  // Per-keystroke delay makes the typing legible in the capture.
  await page.keyboard.type('Evening walk. Cold air, clear head.', { delay: 45 });
  // Autosave debounce is 1200ms; the status pill flips to "Saved at ...".
  await expect(page.getByText(/Saved at|All changes saved/)).toBeVisible({ timeout: 10_000 });
  await page.waitForTimeout(700);
  await page.goto('/dashboard/history');
  await expect(page.getByRole('button', { name: /Open entry from/ }).first()).toBeVisible();
  await page.waitForTimeout(1200);
  await stop();
});

test('gif frames: themes', async ({ page }) => {
  test.setTimeout(180_000);
  await page.setViewportSize(GIF_VIEW);
  let n = 0;
  for (const theme of ['default', 'light', 'dark', 'synthwave']) {
    await setTheme(theme);
    await page.goto('/dashboard');
    await expect(page.locator('.mood-grid').first()).toBeVisible();
    // Let the theme class settle before the shot.
    await page.waitForTimeout(400);
    await page.screenshot({
      path: `${FRAME_DIR}/themes/${String(n++).padStart(2, '0')}-${theme}.png`,
    });
  }
  await setTheme('synthwave');
});
