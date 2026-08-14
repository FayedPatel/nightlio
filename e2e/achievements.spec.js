import { test, expect } from './support/fixtures';
import { wipeEntries, seedEntry, isoDaysAgo } from './support/api';

test.beforeEach(async () => {
  await wipeEntries();
  // A 7-day streak: the server evaluates achievements on each entry create,
  // so the last seed unlocks Week Warrior (and First Entry before it).
  for (let day = 6; day >= 0; day -= 1) {
    await seedEntry({
      date: isoDaysAgo(day),
      mood: 4,
      content: `Streak day ${day}.`,
    });
  }
});

test('all five achievement cards render with rarity badges', async ({ page }) => {
  await page.goto('/dashboard/achievements');
  for (const name of [
    'First Entry',
    'Week Warrior',
    'Consistency King',
    'Data Lover',
    'Mood Master',
  ]) {
    await expect(page.getByText(name, { exact: true })).toBeVisible();
  }
  await expect(page.getByLabel('rarity: legendary')).toBeVisible();
});

test('a 7-day streak unlocks Week Warrior', async ({ page }) => {
  await page.goto('/dashboard/achievements');
  // First Entry + Week Warrior are both unlocked by the seeds.
  await expect(page.getByText('Unlocked')).toHaveCount(2);
});

test('achievement detail modal opens and closes', async ({ page }) => {
  await page.goto('/dashboard/achievements');
  await page.getByText('Consistency King', { exact: true }).click();
  const modal = page.locator('.ui-modal-overlay');
  await expect(modal.getByText('Maintain a 30-day streak')).toBeVisible();
  await expect(modal.getByText('Progress to unlock')).toBeVisible();
  await page.keyboard.press('Escape');
  await expect(modal).not.toBeVisible();
});
