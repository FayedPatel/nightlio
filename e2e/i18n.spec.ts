import { test, expect } from './support/fixtures';
import type { Page } from '@playwright/test';

// v0.6.0 hot-swappable language packs -- Settings > Language section.
//
// The real per-worker Rust API (scripts/e2e-api.sh) ships the /api/i18n
// routes, and the e2e boot exports I18N_OFFLINE=1 with no local packs, so
// against the real, unmocked backend the language list is empty -- the
// no-packs degrade path this suite pins: the app must simply keep working
// in English, exactly as it would offline. Scenarios the hermetic backend
// can't produce (a populated language list, and an outright list-request
// failure) are exercised with page.route() browser-level mocks so the
// frontend's own union/degrade logic is covered independently of whatever
// packs happen to be published; the /api/i18n routes' server-side behavior
// is covered by their own fixture-graded API tests.
//
// Locating the picker by the "English" radio option (rather than by the
// section's heading/aria-label text) is deliberate: "English" is the
// bundled catalog's own entry, always present no matter what packs exist
// or which language is active, so this spec stays stable however the
// section's title copy evolves through t().
const languageRadioGroup = (page: Page) =>
  page.locator('[role="radiogroup"]').filter({ has: page.getByRole('radio', { name: 'English' }) });

test('settings shows the language section with English selected by default', async ({ page }) => {
  await page.goto('/dashboard/settings');

  const group = languageRadioGroup(page);
  await expect(group).toBeVisible();
  await expect(group.getByRole('radio', { name: 'English' })).toHaveAttribute('aria-checked', 'true');
});

test('appearance picker still renders untouched alongside the new language section', async ({ page }) => {
  await page.goto('/dashboard/settings');

  // Pins that the new section was inserted without disturbing the existing
  // Appearance radiogroup (existing English text assertions stay valid).
  await expect(page.getByRole('heading', { name: 'Appearance' })).toBeVisible();
  const themeGroup = page.getByRole('radiogroup', { name: 'Theme' });
  for (const label of ['Default', 'Light', 'Dark', 'Synthwave']) {
    await expect(themeGroup.getByRole('radio', { name: label })).toBeVisible();
  }

  // Both radiogroups coexist -- the theme group is not the language group.
  await expect(languageRadioGroup(page)).toBeVisible();
});

test('language list degrades cleanly when the API returns an empty list', async ({ page }) => {
  await page.route('**/api/i18n/languages', (route) =>
    route.fulfill({ json: { languages: [] } }),
  );

  await page.goto('/dashboard/settings');

  const group = languageRadioGroup(page);
  await expect(group.getByRole('radio')).toHaveCount(1);
  await expect(group.getByRole('radio', { name: 'English' })).toHaveAttribute('aria-checked', 'true');
});

test('language list degrades cleanly when the API is unreachable', async ({ page }) => {
  await page.route('**/api/i18n/languages', (route) => route.abort('failed'));

  await page.goto('/dashboard/settings');

  const group = languageRadioGroup(page);
  await expect(group.getByRole('radio')).toHaveCount(1);
  await expect(group.getByRole('radio', { name: 'English' })).toHaveAttribute('aria-checked', 'true');
});

test('language picker unions bundled English with a populated server list', async ({ page }) => {
  await page.route('**/api/i18n/languages', (route) =>
    route.fulfill({
      json: {
        languages: [
          { code: 'es', name: 'Spanish', native_name: 'Español', version: '1.0.0' },
          { code: 'fr', name: 'French', native_name: 'Français', version: '1.2.0' },
        ],
      },
    }),
  );

  await page.goto('/dashboard/settings');

  const group = languageRadioGroup(page);
  await expect(group.getByRole('radio')).toHaveCount(3);
  await expect(group.getByRole('radio', { name: 'Español' })).toBeVisible();
  await expect(group.getByRole('radio', { name: 'Français' })).toBeVisible();
  // English stays the active selection -- listing a language is not the
  // same as fetching/applying its pack (src/i18n/sync.tsx only fetches a
  // pack for the language actually chosen via setLang).
  await expect(group.getByRole('radio', { name: 'English' })).toHaveAttribute('aria-checked', 'true');
});

test('choosing a listed language persists the selection via the i18n runtime', async ({ page }) => {
  await page.route('**/api/i18n/languages', (route) =>
    route.fulfill({
      json: { languages: [{ code: 'es', name: 'Spanish', native_name: 'Español', version: '1.0.0' }] },
    }),
  );
  // Only the languages list is mocked -- the real backend has no "es" pack
  // cached (I18N_OFFLINE=1), so picking the language must not crash the
  // page even though the pack fetch behind it 404s.
  await page.goto('/dashboard/settings');

  const group = languageRadioGroup(page);
  await group.getByRole('radio', { name: 'Español' }).click();
  await expect(group.getByRole('radio', { name: 'Español' })).toHaveAttribute('aria-checked', 'true');
  await expect(group.getByRole('radio', { name: 'English' })).toHaveAttribute('aria-checked', 'false');

  // Persisted in localStorage (nightlio:lang:v1), like ThemeContext's theme
  // choice -- survives a reload with no flash of the wrong selection.
  await page.reload();
  const reloadedGroup = page.locator('[role="radiogroup"]').filter({ has: page.getByRole('radio', { name: 'Español' }) });
  await expect(reloadedGroup.getByRole('radio', { name: 'Español' })).toHaveAttribute('aria-checked', 'true');
});
