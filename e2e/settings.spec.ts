import { test, expect } from './support/fixtures';
import { seedEntry, isoDaysAgo, listEntries } from './support/api';

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

test('an editing session is one activity row linking to its History card', async ({ page }) => {
  // One-line entry: its title is the first word, so keep that word unique.
  const title = `Session${Date.now()}`;
  const contentOf = async () =>
    (await listEntries()).find((entry) => entry.content.startsWith(title))?.content ?? '';

  await page.goto('/dashboard');
  await page.locator('.mood-grid').first().getByTitle('Good').click();
  const editor = page.locator('.mdx-editor [contenteditable]').first();
  await editor.click();
  // Three autosaves (1200 ms debounce) in one editor mount: POST, PUT, PUT.
  for (const words of [`${title} first`, ' second', ' third']) {
    await page.keyboard.type(words);
    await expect.poll(contentOf, { timeout: 10_000 }).toContain(words.trim());
  }
  const entry = (await listEntries()).find((item) => item.content.startsWith(title));

  await page.goto('/dashboard/settings');
  const rows = page
    .getByRole('region', { name: 'Recent activity' })
    .getByRole('listitem')
    .filter({ hasText: title });
  await expect(rows).toHaveCount(1);
  await expect(rows).toContainText('Created a journal entry');

  await rows.getByRole('link', { name: title }).click();
  await expect(page).toHaveURL(/\/dashboard\/history$/);
  await expect(page.locator(`[data-entry-id="${entry?.id}"]`)).toHaveClass(
    /entry-card-row--highlight/,
  );
});

test('mood music dock is absent when the flag is off', async ({ page }) => {
  await page.goto('/dashboard');
  await expect(page.locator('.mood-grid').first()).toBeVisible();
  await expect(page.locator('.music-dock')).toHaveCount(0);
});
