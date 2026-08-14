import { test, expect } from './support/fixtures';
import { wipeEntries, listEntries } from './support/api';

test('header button cycles through all four themes and persists', async ({ page }) => {
  await page.goto('/dashboard');
  const html = page.locator('html');
  await expect(html).toHaveAttribute('data-theme', 'default');

  const toggle = page.getByLabel('Toggle theme');
  await toggle.click();
  await expect(html).toHaveAttribute('data-theme', 'light');
  await toggle.click();
  await expect(html).toHaveAttribute('data-theme', 'dark');
  await toggle.click();
  await expect(html).toHaveAttribute('data-theme', 'synthwave');

  await page.reload();
  await expect(html).toHaveAttribute('data-theme', 'synthwave');

  await page.getByLabel('Toggle theme').click();
  await expect(html).toHaveAttribute('data-theme', 'default');
});

test('settings theme picker saves the choice to the account', async ({ page, request, apiPort }) => {
  await page.goto('/dashboard/settings');
  const picker = page.getByRole('radiogroup', { name: 'Theme' });
  await expect(picker.getByRole('radio')).toHaveCount(4);

  await picker.getByRole('radio', { name: 'Synthwave' }).click();
  await expect(page.locator('html')).toHaveAttribute('data-theme', 'synthwave');
  await expect(picker.getByRole('radio', { name: 'Synthwave' })).toHaveAttribute(
    'aria-checked',
    'true',
  );

  // The preference is stored server-side, not just in this browser.
  const login = await request.post(`http://localhost:${apiPort}/api/auth/local/login`);
  const { token } = await login.json();
  const prefs = await request.get(`http://localhost:${apiPort}/api/preferences`, {
    headers: { Authorization: `Bearer ${token}` },
  });
  expect((await prefs.json()).theme).toBe('synthwave');

  // Leave the account on the default theme for later specs.
  await picker.getByRole('radio', { name: 'Default' }).click();
  await expect(page.locator('html')).toHaveAttribute('data-theme', 'default');
});

test('burner mode disables saving in the editor', async ({ page }) => {
  await wipeEntries();
  await page.goto('/dashboard');

  await page.getByLabel('Toggle burner mode').click();
  await expect(page.getByLabel('Toggle burner mode')).toHaveAttribute(
    'aria-pressed',
    'true',
  );

  await page.locator('.mood-grid').first().getByTitle('Okay').click();
  await expect(page).toHaveURL(/\/dashboard\/entry$/);
  await expect(
    page.getByText('Saving is turned off in burner mode.'),
  ).toBeVisible();
  await expect(page.getByRole('button', { name: 'Discard' })).toHaveCount(0);

  // Typing must not create an entry (autosave debounce is 1200 ms).
  const editor = page.locator('.mdx-editor [contenteditable]').first();
  await editor.click();
  await page.keyboard.type('This must never be saved.');
  await page.waitForTimeout(2500);
  expect(await listEntries()).toHaveLength(0);
});
