import { readFileSync } from 'node:fs';
import type { Page } from '@playwright/test';

import { test, expect } from './fixtures';

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

test('a wrong password says so @forms', async ({ page }) => {
  await page.goto('/');
  await page.getByLabel('Password').fill('not the password at all');
  await page.getByRole('button', { name: 'Sign in' }).click();

  await expect(page.getByRole('alert')).toContainText('The username or the password is wrong.');
});

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
