import { readFileSync } from 'node:fs';
import AxeBuilder from '@axe-core/playwright';
import type { Page } from '@playwright/test';

import { test, expect, AXE_TAGS } from './fixtures';

/**
 * Each way of signing in, against a server running in that mode:
 * `npm run test:e2e:auth` serves `forms` for the specs tagged @forms, then
 * `oidc`, with a stand-in provider, for those tagged @oidc. The page holds no
 * API key in either, so it opens on the sign-in screen as a browser would.
 */

/** Written beside the database at first start, where an operator finds it too. */
function generatedPassword(): string {
  return readFileSync(process.env.ROUTARR_E2E_PASSWORD_FILE ?? '', 'utf-8').trim();
}

/**
 * Drawn from a status the server answered, so only once the session holds.
 * The navigation is no proof: the shell draws it while its first status is
 * on the way, before a refusal puts the sign-in screen in its place.
 */
const signedIn = (page: Page) => page.getByRole('status', { name: 'Dry-run: writes blocked' });

test('the generated password opens the application @forms', async ({ page }) => {
  await page.goto('/');
  await page.getByLabel('Password').fill(generatedPassword());
  await page.getByRole('button', { name: 'Sign in' }).click();

  await expect(signedIn(page)).toBeVisible();
});

/**
 * The browser leaves for the provider and comes back with a code the server
 * trades for the token, in a callback that finds the attempt this browser
 * started. A status answered behind the gate is the proof that every step held.
 */
test('the provider signs the browser in @oidc', async ({ page }) => {
  await page.goto('/');
  await page.getByRole('link', { name: 'Sign in with your provider' }).click();

  await expect(signedIn(page)).toBeVisible();
});

/**
 * A member of the read group reads and changes nothing: the shell says so, offers none of the
 * owner's screens, and a screen keeps its rows and drops its writes.
 */
test('a member of the read group signs in to read only @oidc', async ({ page, request }) => {
  await request.post(`${process.env.ROUTARR_E2E_OIDC}/next`, {
    data: { sub: 'e2e-reader', groups: ['e2e-family'] },
  });
  await page.goto('/');
  await page.getByRole('link', { name: 'Sign in with your provider' }).click();

  await expect(page.getByRole('status', { name: 'Read only' })).toBeVisible();
  await expect(page.getByRole('link', { name: 'Settings', exact: true })).toHaveCount(0);
  await page.getByRole('link', { name: 'Rules', exact: true }).click();
  await expect(page.getByRole('heading', { level: 1, name: 'Rules' })).toBeVisible();
  await expect(page.getByRole('button', { name: 'New rule' })).toHaveCount(0);
});

/**
 * The first screen every new user sees, in each mode: the one the sweep of the
 * application screens never reaches, since it opens behind a session.
 */
for (const mode of ['forms', 'oidc']) {
  test(`the sign-in screen passes axe @${mode}`, async ({ page }) => {
    await page.goto('/');
    await expect(page.getByRole('heading', { name: 'Sign in' })).toBeVisible();

    const results = await new AxeBuilder({ page }).withTags(AXE_TAGS).analyze();
    expect(results.violations.map((v) => `${v.id}: ${v.help}`)).toEqual([]);
  });
}
