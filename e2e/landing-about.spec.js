import { test, expect } from '@playwright/test';

test('landing nav, sections, and CTAs', async ({ page }) => {
  await page.goto('/');
  await expect(
    page.getByRole('heading', { name: /privacy-first mood tracker/i }),
  ).toBeVisible();

  // Section anchors exist and the features link scrolls to them.
  await page.getByRole('link', { name: 'Features', exact: true }).click();
  await expect(page.locator('#features')).toBeInViewport();
  await expect(page.locator('#self-host')).toBeAttached();

  // Hero CTA routes to login.
  await page.getByRole('link', { name: 'Get started' }).first().click();
  await expect(page).toHaveURL(/\/login$/);
});

test('about page renders and is reachable from landing', async ({ page }) => {
  await page.goto('/');
  await page
    .getByRole('navigation')
    .getByRole('link', { name: 'About', exact: true })
    .click();
  await expect(page).toHaveURL(/\/about$/);
  await expect(page.getByRole('heading', { level: 1 })).toBeVisible();
});

test('about page privacy/terms links fall through to 404 (known gap)', async ({
  page,
}) => {
  // Documents current behavior: /privacy and /terms are linked from the
  // About page but have no routes. If this test starts failing, the links
  // gained real pages — update it.
  await page.goto('/privacy');
  await expect(page.getByText(/404|not found/i).first()).toBeVisible();
});
