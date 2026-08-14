import { test, expect } from './support/fixtures';

test('unknown top-level routes render NotFound', async ({ page }) => {
  await page.goto('/definitely-not-a-page');
  await expect(page.getByText(/not found|404/i).first()).toBeVisible();
});

test('unknown dashboard routes bounce back home', async ({ page }) => {
  await page.goto('/dashboard/bogus-subpage');
  await expect(page).toHaveURL(/\/dashboard$/);
});
