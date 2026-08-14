// Visual capture utility, not a test: SCREENSHOTS=1 yarn playwright test
// e2e/screenshots.spec.js --project=chromium
// Walks the app's main screens at three viewport widths (desktop, Pixel-7
// class, S25-Ultra class) and saves PNGs under screenshots/ (gitignored).
// Skipped entirely in normal runs and CI.
import { test, expect } from './support/fixtures';
import { wipeEntries, seedEntry, isoDaysAgo } from './support/api';

test.skip(!process.env.SCREENSHOTS, 'set SCREENSHOTS=1 to capture');

const VIEWPORTS = [
  { name: 'desktop-1280', width: 1280, height: 800 },
  { name: 's25-ultra-620', width: 620, height: 1340 },
  { name: 'pixel7-412', width: 412, height: 915 },
];

test.beforeAll(async () => {
  await wipeEntries();
  for (let day = 0; day < 7; day += 1) {
    await seedEntry({
      date: isoDaysAgo(day),
      mood: (day % 5) + 1,
      content: `# Day ${day} ago\nSeeded for screenshots.`,
    });
  }
});

for (const vp of VIEWPORTS) {
  test(`capture ${vp.name}`, async ({ page }) => {
    test.setTimeout(120_000);
    await page.setViewportSize({ width: vp.width, height: vp.height });
    const shot = (name, options = {}) =>
      page.screenshot({ path: `screenshots/${vp.name}/${name}.png`, ...options });

    await page.goto('/');
    await expect(
      page.getByRole('heading', { name: /your moods/i }),
    ).toBeVisible();
    await shot('1-landing', { fullPage: true });

    await page.goto('/dashboard');
    await expect(page.locator('.mood-grid').first()).toBeVisible();
    await shot('2-dashboard');

    await page.goto('/dashboard/history');
    await expect(
      page.getByRole('button', { name: /Open entry from/ }).first(),
    ).toBeVisible();
    await shot('3-history');

    await page.goto('/dashboard');
    await page.locator('.mood-grid').first().getByTitle('Good').click();
    await expect(page).toHaveURL(/\/dashboard\/entry$/);
    await expect(page.getByLabel('Entry date')).toBeVisible();
    await shot('4-entry-editor');

    await page.goto('/dashboard/stats');
    await expect(page.locator('.recharts-surface').first()).toBeVisible({
      timeout: 15_000,
    });
    await shot('5-stats', { fullPage: true });
  });
}
