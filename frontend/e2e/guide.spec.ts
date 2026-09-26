import AxeBuilder from '@axe-core/playwright';

import { test, expect, api } from './fixtures';

/**
 * The getting-started guide.
 *
 * The fixture leaves the installation set up and the guide done, the state every
 * other journey tests. These put the guide back to pending first, and the next
 * test's fixture restores it.
 */
async function showGuide(): Promise<void> {
  await api('/onboarding', { method: 'PUT', body: JSON.stringify({ state: 'pending' }) });
}

test('greets an installation not set up with its next steps', async ({ page, instanceId }) => {
  expect(instanceId).toBeTruthy();
  await showGuide();
  await page.goto('/');

  const guide = page.getByRole('region', { name: 'Getting started' });
  await expect(guide).toBeVisible();
  await expect(guide.getByRole('listitem')).toHaveCount(6);
  // The fixture's instance is synced, so the first step ticked itself.
  await expect(guide.getByRole('listitem').first()).toContainText('Done');
  await expect(
    page.getByRole('link', { name: /^Getting started, required steps done: \d of 4$/ }),
  ).toBeVisible();
});

test('can be skipped, and offers itself back until the setup is done', async ({
  page,
  instanceId,
}) => {
  expect(instanceId).toBeTruthy();
  await showGuide();
  await page.goto('/');

  await page.getByRole('button', { name: 'Skip the guide' }).click();
  await expect(page.getByRole('region', { name: 'Getting started' })).toHaveCount(0);
  await expect(
    page.getByRole('link', { name: /^Getting started, required steps done/ }),
  ).toHaveCount(0);

  await page.getByRole('button', { name: 'Resume the guide' }).click();
  await expect(page.getByRole('region', { name: 'Getting started' })).toBeVisible();
});

test('a step opens the screen that does it, with its dialog ready', async ({
  page,
  instanceId,
}) => {
  expect(instanceId).toBeTruthy();
  await showGuide();
  await page.goto('/');

  await page.getByRole('link', { name: 'Create a rule' }).click();

  await expect(page.getByRole('dialog')).toBeVisible();
  // Taken out of the address, so a reload does not open the editor again.
  await expect(page).toHaveURL(/\/rules$/);
});

test('adding an instance ticks the first step and leads straight to the next', async ({
  page,
  instanceId,
}) => {
  // A fresh installation has no instance. The next test's fixture adds it back.
  await api(`/instances/${instanceId}`, { method: 'DELETE' });
  await showGuide();
  await page.goto('/');

  await page.getByRole('link', { name: 'Add an instance' }).click();
  const dialog = page.getByRole('dialog');
  await dialog.getByLabel('Name', { exact: true }).fill('Radarr');
  await dialog
    .getByLabel('Base URL', { exact: true })
    .fill(process.env.ROUTARR_E2E_ARR ?? 'http://127.0.0.1:7979');
  await dialog.getByLabel('API key', { exact: true }).fill('e2e-key');
  await dialog.getByRole('button', { name: 'Add instance' }).click();

  // Synced on save, not at the scheduler's next pass.
  await expect(page.getByText(/^Radarr is synced\./)).toBeVisible();
  await expect(
    page.getByText('Step 1 of 4 is done. Next: Map root folders to categories'),
  ).toBeVisible();
  await page.getByRole('link', { name: 'Open root folders' }).click();

  await expect(page).toHaveURL(/\/root-folders$/);
  await expect(
    page.getByText('Getting started, step 2 of 4: Map root folders to categories'),
  ).toBeVisible();
});

test('the guide passes axe at WCAG 2.1 AA', async ({ page, instanceId }) => {
  expect(instanceId).toBeTruthy();
  await showGuide();
  await page.goto('/');
  await expect(page.getByRole('region', { name: 'Getting started' })).toBeVisible();

  const results = await new AxeBuilder({ page })
    .withTags(['wcag2a', 'wcag2aa', 'wcag21a', 'wcag21aa', 'best-practice'])
    .analyze();

  expect(results.violations.map((v) => `${v.id}: ${v.help}`)).toEqual([]);
});
