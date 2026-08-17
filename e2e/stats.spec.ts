import { test, expect } from './support/fixtures';
import { wipeEntries, seedEntry, isoDaysAgo } from './support/api';
import type { MoodValue } from '../src/types/api';

test.beforeEach(async () => {
  await wipeEntries();
  for (let day = 0; day < 7; day += 1) {
    await seedEntry({
      date: isoDaysAgo(day),
      // (day % 5) + 1 is always 1..5; TS cannot narrow the arithmetic.
      mood: (((day % 5) + 1) as MoodValue),
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
