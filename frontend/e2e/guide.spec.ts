import AxeBuilder from '@axe-core/playwright';

import { test, expect, api, ARR, AXE_TAGS } from './fixtures';

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

test('greets an installation not set up with its next steps', async ({ page }) => {
  await showGuide();
  await page.goto('/');

  const guide = page.getByRole('region', { name: 'Getting started' });
  await expect(guide).toBeVisible();
  // The fixture's instance is synced, so the first step ticked itself and folded away.
  const steps = guide.getByRole('listitem');
  await expect(steps.first()).toBeVisible();
  expect(await steps.count()).toBeLessThan(6);
  await guide.getByRole('button', { name: 'Show the steps done' }).click();
  await expect(steps).toHaveCount(6);
  await expect(steps.first()).toContainText('Done');
  await expect(
    page.getByRole('link', { name: /^\d\/4 Getting started, required steps done: \d of 4$/ }),
  ).toBeVisible();
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
  await dialog.getByLabel('Base URL', { exact: true }).fill(ARR);
  await dialog.getByLabel('API key', { exact: true }).fill('e2e-key');
  await dialog.getByRole('button', { name: 'Add instance' }).click();

  // Synced on save, not at the scheduler's next pass.
  await expect(page.getByText(/^Radarr is synced\./)).toBeVisible();
  await expect(
    page.getByText('Step 1 of 4 is done. Next: Map root folders to categories'),
  ).toBeVisible();
  await page.getByRole('link', { name: 'Open categories and folders' }).click();

  await expect(page).toHaveURL(/\/categories$/);
  await expect(
    page.getByText('Getting started, step 2 of 4: Map root folders to categories'),
  ).toBeVisible();
});

test('the guide passes axe at WCAG 2.2 AA', async ({ page }) => {
  await showGuide();
  await page.goto('/');
  await expect(page.getByRole('region', { name: 'Getting started' })).toBeVisible();

  const results = await new AxeBuilder({ page }).withTags(AXE_TAGS).analyze();

  expect(results.violations.map((v) => `${v.id}: ${v.help}`)).toEqual([]);
});
