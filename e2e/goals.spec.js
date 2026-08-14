import { test, expect } from '@playwright/test';
import { wipeGoals, listGoals } from './support/api';

test.beforeEach(async () => {
  await wipeGoals();
});

test('create a goal through the form', async ({ page }) => {
  await page.goto('/dashboard/goals');
  await page.getByRole('button', { name: 'Add Goal' }).click();
  await expect(
    page.getByRole('heading', { name: 'Add New Goal' }),
  ).toBeVisible();

  await page.getByLabel('Goal Title *').fill('Evening Walk');
  await page.getByLabel('Description *').fill('Thirty minutes around the block.');
  await page.getByRole('button', { name: '5', exact: true }).click();
  await page.getByRole('button', { name: 'Create Goal' }).click();

  await expect(page.getByText('Evening Walk')).toBeVisible();
  await expect(page.getByText('5 days a week')).toBeVisible();

  const goals = await listGoals();
  expect(goals.some((g) => g.title === 'Evening Walk')).toBe(true);
});

test('quick suggestion prefills the form', async ({ page }) => {
  await page.goto('/dashboard/goals');
  await page.getByRole('button', { name: 'Add Goal' }).click();
  await page.getByRole('button', { name: 'Morning Meditation' }).click();
  await expect(page.getByLabel('Goal Title *')).toHaveValue('Morning Meditation');
});

test('mark a goal as done for today', async ({ page }) => {
  await page.goto('/dashboard/goals');
  await page.getByRole('button', { name: 'Add Goal' }).click();
  await page.getByLabel('Goal Title *').fill('Read Before Bed');
  await page.getByLabel('Description *').fill('One chapter minimum.');
  await page.getByRole('button', { name: 'Create Goal' }).click();

  await page.getByRole('button', { name: 'Mark as done', exact: true }).click();
  await expect(page.getByText('Progress updated!')).toBeVisible();
  await expect(
    page.getByRole('button', { name: 'Completed', exact: true }),
  ).toBeDisabled();
});

test('goal statistics calendar opens from the card', async ({ page }) => {
  await page.goto('/dashboard/goals');
  await page.getByRole('button', { name: 'Add Goal' }).click();
  await page.getByLabel('Goal Title *').fill('Stretching Routine');
  await page.getByLabel('Description *').fill('Morning stretches.');
  await page.getByRole('button', { name: 'Create Goal' }).click();

  await page.getByText('Stretching Routine').click();
  const modal = page.locator('.ui-modal-overlay');
  await expect(modal.getByText('Goal Statistics')).toBeVisible();
  await expect(modal.getByText('Sun')).toBeVisible();
  await page.keyboard.press('Escape');
  await expect(modal).not.toBeVisible();
});

test('delete a goal with confirmation', async ({ page }) => {
  await page.goto('/dashboard/goals');
  await page.getByRole('button', { name: 'Add Goal' }).click();
  await page.getByLabel('Goal Title *').fill('Drink Water');
  await page.getByLabel('Description *').fill('Eight glasses.');
  await page.getByRole('button', { name: 'Create Goal' }).click();
  await expect(page.getByText('Drink Water')).toBeVisible();

  page.on('dialog', (dialog) => dialog.accept());
  await page.getByLabel('Delete goal').click();
  await expect(page.getByText('Goal deleted successfully')).toBeVisible();
  await expect(page.getByText('Drink Water')).not.toBeVisible();
});
