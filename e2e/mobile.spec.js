// iPhone-viewport checks: bottom nav is the mobile navigation, and the old
// purple scroll-to-top FAB must stay gone.
import { test, expect } from '@playwright/test';
import { wipeEntries } from './support/api';

test.beforeEach(async () => {
  await wipeEntries();
});

test('mobile dashboard shows bottom nav and no FAB', async ({ page }) => {
  await page.goto('/dashboard');
  await expect(page.locator('.mood-grid').first()).toBeVisible();
  await expect(page.locator('.bottom-nav')).toBeVisible();
  await expect(page.locator('.fab')).toHaveCount(0);
});

test('an entry can be created from a phone', async ({ page }) => {
  await page.goto('/dashboard');
  await page.locator('.mood-grid').first().getByTitle('Amazing').click();
  await expect(page).toHaveURL(/\/dashboard\/entry$/);
  await expect(page.getByLabel('Entry date')).toBeVisible();
});

test('content clears the fixed bottom nav', async ({ page }) => {
  // Regression: .app-main used to keep a 16px bottom padding on mobile, so
  // the last card scrolled to a stop underneath the fixed bottom nav and
  // its buttons could not be tapped.
  await page.goto('/dashboard');
  await expect(page.locator('.bottom-nav')).toBeVisible();
  const navHeight = await page
    .locator('.bottom-nav')
    .evaluate((el) => el.getBoundingClientRect().height);
  const mainPadding = await page
    .locator('.app-main')
    .evaluate((el) => parseFloat(getComputedStyle(el).paddingBottom));
  expect(mainPadding).toBeGreaterThanOrEqual(navHeight);
});
