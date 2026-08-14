import { test, expect } from '@playwright/test';
import { wipeEntries, listEntries } from './support/api';

test('theme toggle flips data-theme and persists across reload', async ({ page }) => {
  await page.goto('/dashboard');
  await expect(page.locator('html')).toHaveAttribute('data-theme', 'dark');

  await page.getByLabel('Toggle theme').click();
  await expect(page.locator('html')).toHaveAttribute('data-theme', 'light');

  await page.reload();
  await expect(page.locator('html')).toHaveAttribute('data-theme', 'light');

  await page.getByLabel('Toggle theme').click();
  await expect(page.locator('html')).toHaveAttribute('data-theme', 'dark');
});

test('burner mode disables saving in the editor', async ({ page }) => {
  await wipeEntries();
  await page.goto('/dashboard');

  await page.getByLabel('Toggle burner mode').click();
  await expect(page.getByLabel('Toggle burner mode')).toHaveAttribute(
    'aria-pressed',
    'true',
  );

  await page.locator('.mood-grid').first().getByTitle('Okay').click();
  await expect(page).toHaveURL(/\/dashboard\/entry$/);
  await expect(
    page.getByText('Saving is turned off in burner mode.'),
  ).toBeVisible();
  await expect(page.getByRole('button', { name: 'Discard' })).toHaveCount(0);

  // Typing must not create an entry (autosave debounce is 1200 ms).
  const editor = page.locator('.mdx-editor [contenteditable]').first();
  await editor.click();
  await page.keyboard.type('This must never be saved.');
  await page.waitForTimeout(2500);
  expect(await listEntries()).toHaveLength(0);
});
