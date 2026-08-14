import { test, expect } from '@playwright/test';
import { wipeEntries, seedEntry, isoDaysAgo } from './support/api';

test.beforeEach(async () => {
  await wipeEntries();
  for (let day = 0; day < 7; day += 1) {
    await seedEntry({
      date: isoDaysAgo(day),
      mood: (day % 5) + 1,
      content: `Entry ${day} days ago.`,
    });
  }
});

test('statistics page renders with seeded data', async ({ page }) => {
  await page.goto('/dashboard/stats');
  await expect(page).toHaveURL(/\/dashboard\/stats/);
  // recharts renders SVG surfaces once statistics load.
  await expect(page.locator('.recharts-surface').first()).toBeVisible({
    timeout: 15_000,
  });
});
