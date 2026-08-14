import { test, expect } from '@playwright/test';

test('landing page renders the marketing shell', async ({ page }) => {
  await page.goto('/');
  await expect(
    page.getByRole('heading', { name: /your moods/i }),
  ).toBeVisible();
  await expect(
    page.getByRole('link', { name: 'Sign in' }).first(),
  ).toBeVisible();
});

test('visiting /dashboard auto-logs-in (self-host mode)', async ({ page }) => {
  await page.goto('/dashboard');
  await expect(page).toHaveURL(/\/dashboard/);
  // The home mood picker is the signature of an authenticated dashboard.
  await expect(page.locator('.mood-grid').first()).toBeVisible();
});
