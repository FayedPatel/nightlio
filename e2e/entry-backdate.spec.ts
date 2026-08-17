// Regression: new entries used to hardcode today's date; the entry-date row
// must let the user file an entry under a past day.
import { test, expect } from './support/fixtures';
import { wipeEntries, listEntries, isoDaysAgo } from './support/api';

test.beforeEach(async () => {
  await wipeEntries();
});

test('an entry can be backdated to yesterday', async ({ page }) => {
  await page.goto('/dashboard');
  await page.locator('.mood-grid').first().getByTitle('Okay').click();
  await expect(page).toHaveURL(/\/dashboard\/entry$/);

  await page.getByRole('button', { name: 'Yesterday' }).click();

  const editor = page.locator('.mdx-editor [contenteditable]').first();
  await editor.click();
  await page.keyboard.press('ControlOrMeta+a');
  await page.keyboard.type('Catching up on yesterday.');
  await expect(page.getByText(/Saved at|All changes saved/)).toBeVisible({
    timeout: 10_000,
  });

  const entries = await listEntries();
  expect(entries).toHaveLength(1);
  expect(entries[0]?.date).toBe(isoDaysAgo(1));
});

test('the date input rejects future dates', async ({ page }) => {
  await page.goto('/dashboard');
  await page.locator('.mood-grid').first().getByTitle('Okay').click();

  const input = page.getByLabel('Entry date');
  await expect(input).toHaveAttribute('max', isoDaysAgo(0));
});
