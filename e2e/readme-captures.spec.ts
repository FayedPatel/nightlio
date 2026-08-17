// README capture utility, not a test: README_CAPTURES=1 yarn playwright test
// e2e/readme-captures.spec.ts --project=chromium
// Seeds a presentable account (a week of markdown entries, goals with
// progress, unlocked achievements), then saves the README's still shots to
// docs/assets/ and GIF keyframes to screenshots/readme-frames/ (assembled
// into GIFs by scripts/build-readme-gifs.mjs). Stills use the synthwave
// theme; long pages and interactive flows are captured as GIF frame
// sequences instead of tall stills. Skipped in normal runs/CI.
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
  // The stats page is the longest in the app — a scroll-through GIF shows
  // the whole thing without a skyscraper still.
  test.setTimeout(180_000);
  await page.setViewportSize(GIF_VIEW);
  await page.goto('/dashboard/stats');
  await expect(page.locator('.recharts-surface').first()).toBeVisible({ timeout: 15_000 });
  await page.waitForTimeout(600);
  const total = await page.evaluate(() => document.documentElement.scrollHeight);
  const step = Math.floor(GIF_VIEW.height * 0.8);
  let n = 0;
  for (let y = 0; y < total; y += step) {
    await page.evaluate(top => window.scrollTo({ top, behavior: 'instant' as ScrollBehavior }), y);
    await page.waitForTimeout(350);
    await page.screenshot({
      path: `${FRAME_DIR}/stats-scroll/${String(n++).padStart(2, '0')}.png`,
    });
  }
});

test('gif frames: log a mood', async ({ page }) => {
  test.setTimeout(180_000);
  await page.setViewportSize(GIF_VIEW);
  let n = 0;
  const frame = () =>
    page.screenshot({ path: `${FRAME_DIR}/log-mood/${String(n++).padStart(2, '0')}.png` });

  await page.goto('/dashboard');
  await expect(page.locator('.mood-grid').first()).toBeVisible();
  await frame();
  await page.locator('.mood-grid').first().getByTitle('Good').hover();
  await frame();
  await page.locator('.mood-grid').first().getByTitle('Good').click();
  await expect(page).toHaveURL(/\/dashboard\/entry$/);
  const editor = page.locator('.mdx-editor [contenteditable]').first();
  await expect(editor).toBeVisible();
  await frame();
  await editor.click();
  await page.keyboard.type('Evening walk. Cold air, clear head.');
  await frame();
  // Autosave debounce is 1200ms; the status pill flips to "Saved at ...".
  await expect(page.getByText(/Saved at|All changes saved/)).toBeVisible({ timeout: 10_000 });
  await frame();
  await page.goto('/dashboard/history');
  await expect(page.getByRole('button', { name: /Open entry from/ }).first()).toBeVisible();
  await frame();
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
