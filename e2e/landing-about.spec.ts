import { test, expect } from './support/fixtures';

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

  // The Sign in button routes into the login flow. In the credential-free
  // harness /login auto-logs-in and bounces to /dashboard, so either URL
  // proves the click worked — asserting only /login races the auto-login.
  await page
    .getByRole('navigation')
    .getByRole('link', { name: 'Sign in' })
    .click();
  await expect(page).toHaveURL(/\/(login|dashboard)$/);
});

test('landing credits the original project and links the fork', async ({ page }) => {
  await page.goto('/');
  const footer = page.locator('.landing__footer');
  await expect(footer.getByText(/© 2026 Nightlio/)).toBeVisible();
  await expect(
    footer.getByRole('link', { name: 'original Nightlio' }),
  ).toHaveAttribute('href', 'https://github.com/shirsakm/nightlio');
  await expect(footer.getByRole('link', { name: 'GitHub' })).toHaveAttribute(
    'href',
    /github\.com\/.+\/nightlio/,
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
  await expect(page.getByText(/this is a fork, and it says so proudly/i)).toBeVisible();
  await expect(
    page.getByRole('heading', { name: /the v0\.4\.0 rewrite/i }),
  ).toBeVisible();
  await expect(
    page.getByText(/rust api built on axum and rusqlite/i),
  ).toBeVisible();
  await expect(
    page.getByRole('link', { name: 'original project on GitHub' }),
  ).toHaveAttribute('href', 'https://github.com/shirsakm/nightlio');
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
