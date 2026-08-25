// Settings → Data: versioned JSON export/import (v0.6.0). The export file is
// built client-side from GET /api/export/data; import posts the same envelope
// back via POST /api/import/data after a local schema_version sanity check.
import { readFile } from 'node:fs/promises';
import { test, expect } from './support/fixtures';
import { wipeEntries, wipeGoals, seedEntry, isoDaysAgo } from './support/api';
import type { DataExport } from '../src/types/api';

test.beforeEach(async () => {
  await wipeEntries();
  await wipeGoals();
});

test('export downloads a versioned JSON file containing the seeded entry', async ({ page }) => {
  await seedEntry({ date: isoDaysAgo(1), mood: 4, content: 'Export me, Playwright.' });

  await page.goto('/dashboard/settings');
  const downloadPromise = page.waitForEvent('download');
  await page.getByRole('button', { name: 'Export data' }).click();
  const download = await downloadPromise;

  expect(download.suggestedFilename()).toMatch(/^nightlio-export-\d{4}-\d{2}-\d{2}\.json$/);

  const parsed: DataExport = JSON.parse(await readFile(await download.path(), 'utf-8'));
  expect(parsed.schema_version).toBe(1);
  expect(parsed.data.entries).toHaveLength(1);
  expect(parsed.data.entries[0]?.content).toBe('Export me, Playwright.');
  expect(parsed.data.entries[0]?.mood).toBe(4);
});

test('import merges an export file and reports per-category counts', async ({ page }) => {
  const envelope: DataExport = {
    schema_version: 1,
    exported_at: '2026-01-01 00:00:00',
    app_version: '0.6.0',
    data: {
      entries: [
        {
          date: isoDaysAgo(2),
          mood: 5,
          // Heading + body so HistoryEntry's title/body split keeps the body
          // sentence in one DOM node (a bare sentence gets word-split apart).
          content: '# Backup note\n\nImported from backup.',
          created_at: '2026-01-01 00:00:00',
          updated_at: '2026-01-01 00:00:00',
          selections: [],
        },
      ],
      goals: [],
    },
  };

  await page.goto('/dashboard/settings');
  await page.locator('input[type="file"]').setInputFiles({
    name: 'nightlio-export-2026-01-01.json',
    mimeType: 'application/json',
    buffer: Buffer.from(JSON.stringify(envelope)),
  });

  await expect(
    page.getByText('Imported 1 entries (0 skipped) and 0 goals (0 skipped).'),
  ).toBeVisible();

  await page.goto('/dashboard/history');
  await expect(page.getByText('Backup note')).toBeVisible();
  await expect(page.getByText('Imported from backup.')).toBeVisible();
});

test('a non-JSON file shows an inline error without calling the import API', async ({ page }) => {
  const importRequests: string[] = [];
  page.on('request', (request) => {
    if (request.url().includes('/api/import/data')) importRequests.push(request.url());
  });

  await page.goto('/dashboard/settings');
  await page.locator('input[type="file"]').setInputFiles({
    name: 'not-an-export.txt',
    mimeType: 'text/plain',
    buffer: Buffer.from('definitely not json'),
  });

  await expect(
    page.getByText('That file does not look like a Nightlio export.'),
  ).toBeVisible();
  expect(importRequests).toHaveLength(0);
});
