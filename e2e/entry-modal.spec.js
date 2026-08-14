import { test, expect } from '@playwright/test';
import { wipeEntries, seedEntry, isoDaysAgo } from './support/api';

test.beforeEach(async () => {
  await wipeEntries();
  await seedEntry({
    date: isoDaysAgo(1),
    mood: 5,
    content: '# Modal fixture\nBody text for the preview.',
  });
});

test('entry preview modal shows title, body, and date', async ({ page }) => {
  await page.goto('/dashboard/history');
  await page.getByRole('button', { name: /Open entry from/ }).first().click();

  const modal = page.locator('.ui-modal-overlay');
  await expect(modal.getByText('Modal fixture')).toBeVisible();
  await expect(modal.getByText('Body text for the preview.')).toBeVisible();
  await page.keyboard.press('Escape');
  await expect(modal).not.toBeVisible();
});

test('export as PDF downloads Entry_<date>.pdf', async ({ page }) => {
  await page.goto('/dashboard/history');
  await page.getByRole('button', { name: /Open entry from/ }).first().click();

  const downloadPromise = page.waitForEvent('download');
  await page.getByLabel('Export as PDF').click();
  const download = await downloadPromise;
  expect(download.suggestedFilename()).toBe(`Entry_${isoDaysAgo(1)}.pdf`);
});

test('delete entry asks for confirmation and removes the card', async ({ page }) => {
  await page.goto('/dashboard/history');
  await page.getByRole('button', { name: /Open entry from/ }).first().click();

  page.on('dialog', (dialog) => dialog.accept());
  await page.getByLabel('Delete entry', { exact: true }).click();
  await expect(page.getByText('Entry deleted')).toBeVisible();
  await expect(
    page.getByRole('button', { name: /Open entry from/ }),
  ).toHaveCount(0);
});

test('declining the confirmation keeps the entry', async ({ page }) => {
  await page.goto('/dashboard/history');
  await page.getByRole('button', { name: /Open entry from/ }).first().click();

  page.on('dialog', (dialog) => dialog.dismiss());
  await page.getByLabel('Delete entry', { exact: true }).click();
  await page.keyboard.press('Escape');
  await expect(
    page.getByRole('button', { name: /Open entry from/ }),
  ).toHaveCount(1);
});
