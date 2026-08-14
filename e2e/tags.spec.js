// Categories (groups) + tag selection on entries. The category manager
// lives inside the entry editor, not Settings.
import { test, expect } from './support/fixtures';
import { wipeEntries, listEntries } from './support/api';

test.beforeEach(async () => {
  await wipeEntries();
});

test('create a category with an option and tag an entry with it', async ({ page }) => {
  await page.goto('/dashboard');
  await page.locator('.mood-grid').first().getByTitle('Good').click();
  await expect(page).toHaveURL(/\/dashboard\/entry$/);

  // Manage Categories: create a category, then an option under it. Names
  // are unique per run-phase since the DB persists across specs.
  await page.getByRole('button', { name: 'Manage Categories' }).click();
  const categoryName = `Weather-${Date.now() % 100000}`;
  await page
    .getByPlaceholder('Category name (e.g., Activities, Weather)')
    .fill(categoryName);
  await page.getByRole('button', { name: 'Create', exact: true }).click();
  await expect(page.getByText(`${categoryName} (0 options)`)).toBeVisible();

  await page.locator('select').selectOption({ label: categoryName });
  await page
    .getByPlaceholder('Option name (e.g., happy, tired)')
    .fill('sunny');
  await page.getByRole('button', { name: 'Add', exact: true }).click();
  await expect(page.getByText(`${categoryName} (1 options)`)).toBeVisible();

  // Select the tag, write content, let autosave fire.
  await page
    .locator('.entry-left')
    .getByRole('button', { name: 'sunny', exact: true })
    .first()
    .click();
  const editor = page.locator('.mdx-editor [contenteditable]').first();
  await editor.click();
  await page.keyboard.press('ControlOrMeta+a');
  await page.keyboard.type('Tagged entry for the tags spec.');
  await expect(page.getByText(/Saved at|All changes saved/)).toBeVisible({
    timeout: 10_000,
  });

  const entries = await listEntries();
  expect(entries).toHaveLength(1);

  // The tag chip shows on the history card.
  await page.goto('/dashboard/history');
  const card = page.getByRole('button', { name: /Open entry from/ }).first();
  await expect(card.locator('.tag', { hasText: 'sunny' })).toBeVisible();
});
