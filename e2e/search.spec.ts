// Header search is desktop-only: Header.css hides .header__search at
// max-width 640px, so this spec is excluded from mobile projects in
// playwright.config.ts. Search is client-side over loaded entries.
import { test, expect } from './support/fixtures';
import { wipeEntries, seedEntry, isoDaysAgo } from './support/api';

test.beforeEach(async () => {
  await wipeEntries();
  await seedEntry({ date: isoDaysAgo(1), mood: 4, content: '# Beach trip\nSunny all day.' });
  await seedEntry({ date: isoDaysAgo(2), mood: 2, content: '# Work stress\nLong meetings.' });
});

test('search filters entries and routes to History', async ({ page }) => {
  await page.goto('/dashboard');
  const input = page.getByPlaceholder('Search...');
  await input.fill('beach');

  await expect(page).toHaveURL(/\/dashboard\/history/);
  await expect(
    page.getByRole('heading', { name: 'Search Results (1)' }),
  ).toBeVisible();
  await expect(page.getByText('Beach trip')).toBeVisible();
  await expect(page.getByText('Work stress')).not.toBeVisible();

  // Clearing restores the full history list.
  await page.locator('.search-bar-clear').click();
  await expect(page.getByRole('heading', { name: 'History' })).toBeVisible();
  await expect(page.getByText('Work stress')).toBeVisible();
});

test('search with no matches shows zero results', async ({ page }) => {
  await page.goto('/dashboard');
  await page.getByPlaceholder('Search...').fill('zzz-no-such-entry');
  await expect(
    page.getByRole('heading', { name: 'Search Results (0)' }),
  ).toBeVisible();
});
