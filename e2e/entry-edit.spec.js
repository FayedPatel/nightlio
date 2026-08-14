// Regression: editing from the History page used to navigate to a
// non-existent nested route and render a blank screen.
import { test, expect } from '@playwright/test';
import { wipeEntries, seedEntry, isoDaysAgo } from './support/api';

test.beforeEach(async () => {
  await wipeEntries();
  await seedEntry({
    date: isoDaysAgo(2),
    mood: 2,
    content: '# Rough day\nLong meetings.',
  });
});

test('edit from the History page opens the editor', async ({ page }) => {
  await page.goto('/dashboard/history');

  // Open the entry's preview modal, then its edit action — stable on both
  // desktop (hover toolbar) and touch (swipe strip) layouts.
  await page.getByRole('button', { name: /Open entry from/ }).first().click();
  await page.getByLabel('Edit entry', { exact: true }).click();

  await expect(page).toHaveURL(/\/dashboard\/entry$/);
  await expect(page.getByText(/Editing entry from/)).toBeVisible();
  await expect(
    page.locator('.mdx-editor [contenteditable]').first(),
  ).toContainText('Rough day');
});

test('saving an edit persists the change', async ({ page }) => {
  await page.goto('/dashboard/history');
  await page.getByRole('button', { name: /Open entry from/ }).first().click();
  await page.getByLabel('Edit entry', { exact: true }).click();

  const editor = page.locator('.mdx-editor [contenteditable]').first();
  await editor.click();
  await page.keyboard.press('End');
  await page.keyboard.type(' Edited by Playwright.');
  await expect(page.getByText(/Saved at|All changes saved/)).toBeVisible({
    timeout: 10_000,
  });

  await page.goto('/dashboard/history');
  await expect(
    page.getByRole('button', { name: /Open entry from/ }).first(),
  ).toContainText('Edited by Playwright.');
});
