import type { Page } from '@playwright/test';

import { test, expect, openScreen, screenShown } from './fixtures';
import { SCREENS } from './screens';

/**
 * Routarr mounted under a sub-path, the Servarr "URL base" convention.
 *
 * Run by `npm run test:e2e:base`, which starts the same server with
 * `ROUTARR_BASE_PATH=/routarr`. Everything here fails in exactly one place,
 * behind a reverse proxy, so a browser is the only witness that counts.
 */
const BASE = '/routarr';

const sidebarLinks = (page: Page) =>
  page.getByRole('navigation', { name: 'Main navigation' }).getByRole('link');

test('a deep link loads its assets rather than coming up blank @subpath', async ({ page }) => {
  const failures: string[] = [];
  page.on('response', (r) => {
    if (!r.ok()) failures.push(`${r.status()} ${r.url()}`);
  });
  page.on('pageerror', (e) => failures.push(`JS: ${e.message}`));

  // Straight to a client-side route: without a `<base href>` the relative asset
  // URLs would resolve against /routarr/rules/ and 404.
  await openScreen(page, `${BASE}/rules`);
  expect(failures, 'nothing may fail to load').toEqual([]);
});

test('navigation keeps the prefix @subpath', async ({ page }) => {
  await page.goto(`${BASE}/`);

  await sidebarLinks(page).nth(1).click();
  await screenShown(page, 'the second sidebar entry');

  // A link that dropped the prefix would leave the proxy entirely.
  expect(new URL(page.url()).pathname.startsWith(`${BASE}/`)).toBe(true);
});

/**
 * A `<base href>` applies to relative URLs only, so a root-absolute
 * `href="/rules"` sends every affordance of a link (a click, a middle-click, a
 * ctrl-click, the status bar, a copied address) to the proxy's root and a 404.
 * The attribute is what the browser reads, so the attribute is what is
 * checked, on every link the shell draws.
 */
test('every link carries the prefix, and a ctrl-click stays under it @subpath', async ({
  page,
  context,
}) => {
  await openScreen(page, `${BASE}/`);

  const hrefs = await page
    .locator('a[href^="/"]')
    .evaluateAll((anchors) => anchors.map((a) => a.getAttribute('href') ?? ''));
  // Every screen's entry at least: a shortfall is a link the check never met.
  expect(hrefs.length).toBeGreaterThanOrEqual(SCREENS.length);
  for (const href of hrefs) {
    expect(href, `${href} leaves the mount point`).toMatch(new RegExp(`^${BASE}(/|$)`));
  }

  const [opened] = await Promise.all([
    context.waitForEvent('page'),
    sidebarLinks(page)
      .nth(1)
      .click({ modifiers: ['Control'] }),
  ]);
  await opened.waitForLoadState();
  expect(new URL(opened.url()).pathname.startsWith(`${BASE}/`)).toBe(true);
  await screenShown(opened, opened.url());
  await opened.close();
});

/** The contract's download is a link of its own, drawn on the reference alone. */
test('the contract downloads through the prefix @subpath', async ({ page }) => {
  await openScreen(page, `${BASE}/reference`);

  const download = page.getByRole('link', { name: 'Download openapi.json' });
  await expect(download).toHaveAttribute('href', `${BASE}/api/v1/openapi.json`);
});

test('the API is reached through the prefix @subpath', async ({ page }) => {
  const calls: string[] = [];
  page.on('request', (r) => {
    if (r.url().includes('/api/v1/')) calls.push(new URL(r.url()).pathname);
  });

  await openScreen(page, `${BASE}/instances`);

  expect(calls.length).toBeGreaterThan(0);
  for (const path of calls) {
    expect(path.startsWith(`${BASE}/api/v1/`), `${path} left the mount point`).toBe(true);
  }
});
