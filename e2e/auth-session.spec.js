import { test, expect } from '@playwright/test';

test('logout returns to login and the session can be re-entered', async ({ page }) => {
  await page.goto('/dashboard');
  await expect(page.locator('.mood-grid').first()).toBeVisible();

  await page.getByRole('button', { name: 'Logout' }).click();
  await expect(page).toHaveURL(/\/login$/);

  // In credential-free self-host mode the app auto-logs-in again on the
  // next protected visit — asserting the form here would race that flow.
  await page.goto('/dashboard');
  await expect(page.locator('.mood-grid').first()).toBeVisible();
});
