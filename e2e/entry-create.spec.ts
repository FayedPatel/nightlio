import { test, expect } from './support/fixtures';
import { wipeEntries, listEntries } from './support/api';

test.beforeEach(async () => {
  await wipeEntries();
});

test('creating an entry via mood pick autosaves it', async ({ page }) => {
  await page.goto('/dashboard');
  await page.locator('.mood-grid').first().getByTitle('Good').click();
  await expect(page).toHaveURL(/\/dashboard\/entry$/);

  const editor = page.locator('.mdx-editor [contenteditable]').first();
  await editor.click();
  await page.keyboard.press('ControlOrMeta+a');
  await page.keyboard.type('Playwright wrote this entry.');

  // Autosave debounce is 1200ms; the status pill flips to "Saved at ...".
  await expect(page.getByText(/Saved at|All changes saved/)).toBeVisible({
    timeout: 10_000,
  });

  const entries = await listEntries();
  expect(entries).toHaveLength(1);
  expect(entries[0]?.content).toContain('Playwright wrote this entry.');
  expect(entries[0]?.mood).toBe(4);
});

test('history Add Entry card leads straight into the editor', async ({ page }) => {
  // Regression: the card used to dispatch an event that just navigated back
  // to Home. Now it goes to the editor's inline mood prompt in one click.
  await page.goto('/dashboard/history');
  await page.getByRole('button', { name: 'Add Entry' }).click();

  await expect(page).toHaveURL(/\/dashboard\/entry$/);
  await expect(
    page.getByText('Pick your mood to start an entry'),
  ).toBeVisible();

  await page.locator('.mood-grid').first().getByTitle('Good').click();
  await expect(page.getByLabel('Entry date')).toBeVisible();
});
