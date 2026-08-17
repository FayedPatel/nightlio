import { test, expect } from './support/fixtures';
import {
  wipeGoals,
  listGoals,
  seedGoal,
  listGoalCompletions,
  isoDaysAgo,
} from './support/api';

test.beforeEach(async () => {
  await wipeGoals();
});

test('home Add Goal card opens the creation form in one click', async ({ page }) => {
  // The dashed Add Goal card only renders on Home once at least one goal
  // exists (empty state shows an "Add First Goal" button instead).
  await seedGoal({ title: 'Existing Goal' });
  await page.goto('/dashboard');
  await page
    .locator('section[aria-label="Active goals"]')
    .getByRole('button', { name: 'Add Goal' })
    .click();

  await expect(page).toHaveURL(/\/dashboard\/goals$/);
  await expect(
    page.getByRole('heading', { name: 'Add New Goal' }),
  ).toBeVisible();

  // The openForm navigation state is consumed with a replace, so a reload
  // shows the normal Goals page rather than re-opening the form.
  await page.reload();
  await expect(
    page.getByRole('heading', { name: 'Add New Goal' }),
  ).not.toBeVisible();
  await expect(page.getByRole('button', { name: 'Add Goal' })).toBeVisible();
});

test('empty-state Add First Goal opens the creation form in one click', async ({ page }) => {
  await page.goto('/dashboard');
  await page.getByRole('button', { name: 'Add First Goal' }).click();

  await expect(page).toHaveURL(/\/dashboard\/goals$/);
  await expect(
    page.getByRole('heading', { name: 'Add New Goal' }),
  ).toBeVisible();
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

test('log a goal completion for a past day', async ({ page }) => {
  await seedGoal({ title: 'Water Plants', frequency: 5 });
  await page.goto('/dashboard/goals');

  // exact: true — the goal card itself is a role=button whose accessible
  // name concatenates its children, so a substring match hits both.
  await page.getByRole('button', { name: 'Log past day for Water Plants', exact: true }).click();
  await page.getByRole('button', { name: 'Yesterday', exact: true }).click();
  await page.getByRole('button', { name: 'Log it', exact: true }).click();
  await expect(page.getByText(/Logged for/)).toBeVisible();

  const yesterday = isoDaysAgo(1);
  const goal = (await listGoals()).find((g) => g.title === 'Water Plants');
  if (!goal) throw new Error('seeded goal "Water Plants" not found');
  const completions = await listGoalCompletions(goal.id);
  expect(completions.some((c) => c.date === yesterday)).toBe(true);

  // The weekly counter only credits days inside the current Monday-based
  // week; on Mondays "yesterday" belongs to last week and stays at 0.
  const now = new Date();
  const monday = new Date(now);
  monday.setDate(now.getDate() - ((now.getDay() + 6) % 7));
  const pad = (v: number) => String(v).padStart(2, '0');
  const mondayIso = `${monday.getFullYear()}-${pad(monday.getMonth() + 1)}-${pad(monday.getDate())}`;
  const expected = yesterday >= mondayIso ? '1/5' : '0/5';
  await expect(page.getByText(expected)).toBeVisible();
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
