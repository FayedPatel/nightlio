// README capture utility, not a test: README_CAPTURES=1 yarn playwright test
// e2e/readme-captures.spec.ts --project=chromium
// Seeds a presentable account (a week of markdown entries, goals with
// progress, unlocked achievements), then captures the README's six GIFs
// as real motion via Chrome's CDP screencast — desktop at 720p (1280x720),
// mobile at the S25-Ultra-class 620x1340 viewport, synthwave theme. Raw
// frames land in screenshots/readme-frames/ and are assembled into
// docs/assets/*.gif by scripts/build-readme-gifs.mjs. Skipped in normal
// runs/CI.
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
import type { Group, MoodValue } from '../src/types/api';

test.skip(!process.env.README_CAPTURES, 'set README_CAPTURES=1 to capture');

const FRAME_DIR = 'screenshots/readme-frames';
const DESKTOP = { width: 1280, height: 720 }; // 720p
const MOBILE = { width: 620, height: 1340 }; // S25-Ultra class

const ENTRIES: Array<{ day: number; mood: MoodValue; content: string }> = [
  { day: 0, mood: 5, content: '# Shipped the rewrite\nEverything green on the first run. Celebrated with a long walk and *way* too much coffee.' },
  { day: 1, mood: 4, content: '# Quiet focus day\nDeep work most of the morning, gym in the evening.\n\n- finished the migration notes\n- 5k on the treadmill' },
  { day: 2, mood: 3, content: '# Middling\nSlept badly, but the afternoon picked up after a proper lunch break.' },
  { day: 3, mood: 4, content: '# Board-game night\nLost twice at Wingspan, still worth it.' },
  { day: 4, mood: 2, content: '# Rough one\nServer alerts at 3am. Wrote down what to automate so it never pages me again.' },
  { day: 5, mood: 4, content: '# Recovery\nSlow morning, long reading session, early night.' },
  { day: 6, mood: 5, content: '# Hike day\nTwelve kilometres of forest trail and zero notifications.' },
];

// Weekly progress dedupes same-day repeats, so `done` is a boolean: half
// the goals are completed today (chip + progress tick), half stay open —
// that mix reads better than every card looking identical.
const GOALS: Array<{ title: string; description: string; frequency: number; done: boolean }> = [
  { title: 'Train 3x a week', description: 'Any workout counts.', frequency: 3, done: true },
  { title: 'Read before bed', description: 'Twenty minutes, no phone.', frequency: 5, done: false },
  { title: 'Cook at home', description: 'Takeout is for Fridays.', frequency: 4, done: true },
  { title: 'Morning pages', description: 'Three sentences before coffee.', frequency: 7, done: false },
  { title: 'Walk outside', description: 'Daylight before noon.', frequency: 5, done: true },
  { title: 'Call someone', description: 'Family or an old friend.', frequency: 2, done: false },
];

const setTheme = async (theme: string) => {
  const { ctx, headers } = await apiContext();
  await ctx.put('/api/preferences', { headers, data: { theme } });
  await ctx.dispose();
};

const listGroups = async (): Promise<Group[]> => {
  const { ctx, headers } = await apiContext();
  const groups: Group[] = await (await ctx.get('/api/groups', { headers })).json();
  await ctx.dispose();
  return groups;
};

// Real-motion capture via Chrome's CDP screencast: Chromium streams a frame
// on every visual change, so flows and scrolls come out smooth instead of a
// keyframe slideshow. Frames are written as NNNN-<ms offset>.png; the GIF
// assembler turns the timestamp gaps into per-frame delays.
const startScreencast = async (page: Page, dir: string, maxWidth: number, maxHeight: number) => {
  mkdirSync(dir, { recursive: true });
  const cdp = await page.context().newCDPSession(page);
  const frames: Array<{ data: string; ts: number }> = [];
  cdp.on('Page.screencastFrame', ev => {
    frames.push({ data: ev.data, ts: ev.metadata.timestamp ?? 0 });
    cdp.send('Page.screencastFrameAck', { sessionId: ev.sessionId }).catch(() => {});
  });
  await cdp.send('Page.startScreencast', { format: 'png', maxWidth, maxHeight, everyNthFrame: 1 });
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

// Steady glide to the bottom of the page: many small instant steps at
// screencast rate read as continuous motion.
const glideToBottom = async (page: Page, steps = 90, stepMs = 55) => {
  const total = await page.evaluate(
    () => document.documentElement.scrollHeight - window.innerHeight,
  );
  for (let i = 1; i <= steps; i += 1) {
    await page.evaluate(top => window.scrollTo(0, top), Math.round((total * i) / steps));
    await page.waitForTimeout(stepMs);
  }
};

// The full "log a mood" story, shared by the desktop and mobile GIFs:
// dashboard → pick a mood → clear the placeholder → write → tag categories
// → scroll back up to the Saved pill → return to the dashboard (which now
// shows the new entry; the assembler holds that last frame ~3s).
const logMoodFlow = async (page: Page) => {
  // The desktop and mobile captures both run this flow against the shared
  // DB — drop any earlier take's entry so the dashboard shows exactly one.
  {
    const { ctx, headers } = await apiContext();
    const entries: Array<{ id: number; content?: string }> = await (
      await ctx.get('/api/moods', { headers })
    ).json();
    for (const e of entries) {
      if (e.content?.includes('Evening walk')) {
        await ctx.delete(`/api/mood/${e.id}`, { headers });
      }
    }
    await ctx.dispose();
  }
  // Tags should match the upbeat entry — prefer positive options, fall
  // back to the first few if the defaults ever change.
  const groups = await listGroups();
  const available = groups.flatMap(g => g.options.map(o => o.name));
  const preferred = ['happy', 'relaxed', 'excited'].filter(n => available.includes(n));
  const optionNames = preferred.length >= 2 ? preferred : available.slice(0, 3);

  await page.waitForTimeout(900);
  await page.locator('.mood-grid').first().getByTitle('Good').hover();
  await page.waitForTimeout(450);
  await page.locator('.mood-grid').first().getByTitle('Good').click();
  await expect(page).toHaveURL(/\/dashboard\/entry$/);
  const editor = page.locator('.mdx-editor [contenteditable]').first();
  await expect(editor).toBeVisible();
  await page.waitForTimeout(700);

  // The editor starts empty with a faded placeholder, so just focus and
  // write the way a person would: "# " turns the first line into an H1
  // (markdown shortcut), Enter drops into a paragraph for the body.
  await editor.click();
  await page.keyboard.type('# ', { delay: 55 });
  await page.keyboard.type('Evening walk', { delay: 55 });
  await page.keyboard.press('Enter');
  await page.waitForTimeout(250);
  await page.keyboard.type('Cold air, clear head. Twenty minutes around the block and the day made sense again.', { delay: 35 });
  await page.waitForTimeout(400);

  // Tag the entry: scroll the category groups into view and pick a few.
  for (const name of optionNames) {
    const option = page.getByRole('button', { name, exact: true }).first();
    await option.scrollIntoViewIfNeeded();
    await page.waitForTimeout(350);
    await option.click();
    await page.waitForTimeout(350);
  }

  // Autosave debounce is 1200ms; the pill flips to "Saved at ..." — glide
  // back up so the capture shows the change landed.
  await expect(page.getByText(/Saved at|All changes saved/)).toBeVisible({ timeout: 10_000 });
  const top = await page.evaluate(() => window.scrollY);
  const steps = Math.max(6, Math.round(top / 120));
  for (let i = steps - 1; i >= 0; i -= 1) {
    await page.evaluate(y => window.scrollTo(0, y), Math.round((top * i) / steps));
    await page.waitForTimeout(50);
  }
  await page.waitForTimeout(900);

  // Client-side return — a hard goto() would paint a white frame.
  await page.getByRole('button', { name: 'Return to dashboard' }).click();
  await expect(page.locator('.mood-grid').first()).toBeVisible();
  await expect(page.getByText('Evening walk').first()).toBeVisible();
  await page.waitForTimeout(1000);
};

test.beforeAll(async () => {
  await wipeEntries();
  await wipeGoals();
  for (const e of ENTRIES) {
    await seedEntry({ date: isoDaysAgo(e.day), mood: e.mood, content: e.content });
  }
  const { ctx, headers } = await apiContext();
  for (const g of GOALS) {
    const created = await seedGoal({ title: g.title, description: g.description, frequency: g.frequency });
    if (g.done) {
      await ctx.post(`/api/goals/${created.id}/progress`, { headers, data: {} });
    }
  }
  await ctx.post('/api/achievements/check', { headers, data: {} });
  await ctx.dispose();
  // README media is captured in the synthwave theme.
  await setTheme('synthwave');
});


test('desktop gif: log a mood', async ({ page }) => {
  test.setTimeout(180_000);
  await page.setViewportSize(DESKTOP);
  await page.goto('/dashboard');
  await expect(page.locator('.mood-grid').first()).toBeVisible();
  const stop = await startScreencast(page, `${FRAME_DIR}/log-mood`, 880, 495);
  await logMoodFlow(page);
  await stop();
});

test('desktop gif: goals scroll', async ({ page }) => {
  test.setTimeout(180_000);
  await page.setViewportSize(DESKTOP);
  await page.goto('/dashboard/goals');
  await expect(page.getByText('Train 3x a week')).toBeVisible();
  await page.waitForTimeout(800);
  const stop = await startScreencast(page, `${FRAME_DIR}/goals-scroll`, 880, 495);
  await page.waitForTimeout(700);
  await glideToBottom(page, 70, 55);
  await page.waitForTimeout(1000);
  await stop();
});

test('desktop gif: statistics scroll', async ({ page }) => {
  test.setTimeout(180_000);
  await page.setViewportSize(DESKTOP);
  await page.goto('/dashboard/stats');
  await expect(page.locator('.recharts-surface').first()).toBeVisible({ timeout: 15_000 });
  await page.waitForTimeout(800);
  const stop = await startScreencast(page, `${FRAME_DIR}/stats-scroll`, 880, 495);
  await page.waitForTimeout(700);
  await glideToBottom(page, 100, 55);
  await page.waitForTimeout(1000);
  await stop();
});

test('mobile gif: log a mood', async ({ page }) => {
  test.setTimeout(180_000);
  await page.setViewportSize(MOBILE);
  await page.goto('/dashboard');
  await expect(page.locator('.mood-grid').first()).toBeVisible();
  const stop = await startScreencast(page, `${FRAME_DIR}/mobile-log-mood`, 420, 908);
  await logMoodFlow(page);
  await stop();
});

test('mobile gif: goals scroll', async ({ page }) => {
  test.setTimeout(180_000);
  await page.setViewportSize(MOBILE);
  await page.goto('/dashboard/goals');
  await expect(page.getByText('Train 3x a week')).toBeVisible();
  await page.waitForTimeout(800);
  const stop = await startScreencast(page, `${FRAME_DIR}/mobile-goals`, 420, 908);
  await page.waitForTimeout(700);
  await glideToBottom(page, 90, 55);
  await page.waitForTimeout(1000);
  await stop();
});

test('mobile gif: statistics scroll', async ({ page }) => {
  test.setTimeout(180_000);
  await page.setViewportSize(MOBILE);
  await page.goto('/dashboard/stats');
  await expect(page.locator('.recharts-surface').first()).toBeVisible({ timeout: 15_000 });
  await page.waitForTimeout(800);
  const stop = await startScreencast(page, `${FRAME_DIR}/mobile-stats`, 420, 908);
  await page.waitForTimeout(700);
  await glideToBottom(page, 120, 55);
  await page.waitForTimeout(1000);
  await stop();
});

