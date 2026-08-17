import { test, expect } from './support/fixtures';
import { seedEntry, isoDaysAgo } from './support/api';

test('settings has the appearance picker with all four themes', async ({ page }) => {
  await page.goto('/dashboard/settings');
  await expect(page.getByRole('heading', { name: 'Appearance' })).toBeVisible();
  const picker = page.getByRole('radiogroup', { name: 'Theme' });
  for (const label of ['Default', 'Light', 'Dark', 'Synthwave']) {
    await expect(picker.getByRole('radio', { name: label })).toBeVisible();
  }
});

test('settings shows server-managed feature flags read-only', async ({ page }) => {
  await page.goto('/dashboard/settings');
  await expect(page.getByRole('heading', { name: 'Feature flags' })).toBeVisible();
  await expect(page.getByLabel('Single Sign-On (OIDC)')).toBeDisabled();
  await expect(page.getByLabel('Mood Music')).toBeDisabled();
  // Harness runs with both features off.
  await expect(page.getByText('(disabled)').first()).toBeVisible();
});

test('recent activity logs journal actions', async ({ page }) => {
  await seedEntry({
    date: isoDaysAgo(0),
    mood: 3,
    content: 'Activity-log fixture.',
  });
  await page.goto('/dashboard/settings');
  await expect(page.getByRole('heading', { name: 'Recent activity' })).toBeVisible();
  await expect(
    page.getByText(/Created a journal entry/).first(),
  ).toBeVisible();
});

test('mood music dock is absent when the flag is off', async ({ page }) => {
  await page.goto('/dashboard');
  await expect(page.locator('.mood-grid').first()).toBeVisible();
  await expect(page.locator('.music-dock')).toHaveCount(0);
});
