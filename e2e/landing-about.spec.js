import { test, expect } from '@playwright/test';

test('landing hero, sections, and sign-in CTA', async ({ page }) => {
  await page.goto('/');
  await expect(
    page.getByRole('heading', { name: /your moods/i }),
  ).toBeVisible();

  // Section anchors exist and the features link scrolls to them.
  await page
    .getByRole('navigation')
    .getByRole('link', { name: 'Features', exact: true })
    .click();
  await expect(page.locator('#features')).toBeInViewport();
  await expect(page.locator('#self-host')).toBeAttached();

  // The Sign in button routes to the login page.
  await page
    .getByRole('navigation')
    .getByRole('link', { name: 'Sign in' })
    .click();
  await expect(page).toHaveURL(/\/login$/);
});

test('landing credits the original project and links the fork', async ({ page }) => {
  await page.goto('/');
  const footer = page.locator('.landing__footer');
  await expect(footer.getByText(/maintained by Fayed Patel/)).toBeVisible();
  await expect(
    footer.getByRole('link', { name: 'original Nightlio' }),
  ).toHaveAttribute('href', 'https://github.com/shirsakm/nightlio');
  await expect(footer.getByRole('link', { name: 'GitHub' })).toHaveAttribute(
    'href',
    'https://github.com/FayedPatel/nightlio',
  );
});

test('about page tells the fork story', async ({ page }) => {
  await page.goto('/');
  await page
    .getByRole('navigation')
    .getByRole('link', { name: 'About', exact: true })
    .click();
  await expect(page).toHaveURL(/\/about$/);
  await expect(
    page.getByRole('heading', { name: /why i forked nightlio/i }),
  ).toBeVisible();
  await expect(page.getByText(/originally created by/i)).toBeVisible();
});

test('features link works from the about page', async ({ page }) => {
  // Regression: the nav used a bare #features anchor, which was dead when
  // browsing /about. It now routes to /#features and scrolls there.
  await page.goto('/about');
  await page
    .getByRole('navigation')
    .getByRole('link', { name: 'Features', exact: true })
    .click();
  await expect(page).toHaveURL(/\/#features$/);
  await expect(page.locator('#features')).toBeInViewport();
});

test('unrouted marketing paths render the 404 page', async ({ page }) => {
  await page.goto('/privacy');
  await expect(page.getByText(/404|not found/i).first()).toBeVisible();
});
