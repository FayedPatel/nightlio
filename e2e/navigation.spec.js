// Navigation chrome differs by layout: sidebar on desktop, bottom nav on
// phones (≤640px). Labels differ too (Statistics/Achievements vs
// Stats/Awards), so the spec branches on viewport width.
import { test, expect } from '@playwright/test';

const DESKTOP_ITEMS = [
  ['History', /\/dashboard\/history$/],
  ['Goals', /\/dashboard\/goals$/],
  ['Statistics', /\/dashboard\/stats$/],
  ['Achievements', /\/dashboard\/achievements$/],
  ['Settings', /\/dashboard\/settings$/],
  ['Home', /\/dashboard$/],
];

const MOBILE_ITEMS = [
  ['History', /\/dashboard\/history$/],
  ['Goals', /\/dashboard\/goals$/],
  ['Stats', /\/dashboard\/stats$/],
  ['Awards', /\/dashboard\/achievements$/],
  ['Settings', /\/dashboard\/settings$/],
  ['Home', /\/dashboard$/],
];

test('primary navigation reaches every section', async ({ page }) => {
  await page.goto('/dashboard');
  await expect(page.locator('.mood-grid').first()).toBeVisible();

  const mobile = page.viewportSize().width <= 640;
  const container = mobile
    ? page.locator('.bottom-nav')
    : page.locator('.sidebar');
  await expect(container).toBeVisible();

  for (const [label, urlPattern] of mobile ? MOBILE_ITEMS : DESKTOP_ITEMS) {
    await container.getByRole('link', { name: label, exact: true }).click();
    await expect(page).toHaveURL(urlPattern);
  }

  // The other chrome is hidden for this layout.
  if (mobile) {
    await expect(page.locator('.sidebar')).toBeHidden();
  } else {
    await expect(page.locator('.bottom-nav')).toBeHidden();
  }
});
