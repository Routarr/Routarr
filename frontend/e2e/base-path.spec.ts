import { test, expect } from './fixtures';

/**
 * Routarr mounted under a sub-path, the Servarr "URL base" convention.
 *
 * Run by `npm run test:e2e:base`, which starts the same server with
 * `ROUTARR_BASE_PATH=/routarr`. Everything here fails in exactly one place — a
 * reverse proxy — so a browser is the only witness that counts.
 */
const BASE = '/routarr';

test('a deep link loads its assets rather than coming up blank @subpath', async ({
  page,
  instanceId,
}) => {
  expect(instanceId).toBeTruthy();
  const failures: string[] = [];
  page.on('response', (r) => {
    if (!r.ok() && !r.url().includes('fonts.g')) failures.push(`${r.status()} ${r.url()}`);
  });
  page.on('pageerror', (e) => failures.push(`JS: ${e.message}`));

  // Straight to a client-side route: without a `<base href>` the relative asset
  // URLs would resolve against /routarr/rules/ and 404.
  await page.goto(`${BASE}/rules`);

  await expect(page.locator('.page-title')).toBeVisible();
  expect(failures, 'nothing may fail to load').toEqual([]);
});

test('navigation keeps the prefix @subpath', async ({ page, instanceId }) => {
  expect(instanceId).toBeTruthy();
  await page.goto(`${BASE}/`);

  await page.locator('.sidebar-nav a').nth(1).click();
  await expect(page.locator('.page-title')).toBeVisible();

  // A link that dropped the prefix would leave the proxy entirely.
  expect(new URL(page.url()).pathname.startsWith(`${BASE}/`)).toBe(true);
});

/**
 * The left click worked before this did, because the router prefixed the mount
 * point on interception. A `<base href>` applies to relative URLs only, so a
 * root-absolute `href="/rules"` sent every other affordance of a link — a
 * middle-click, a ctrl-click, the status bar, a copied address — to the
 * proxy's root and a 404. The attribute is what the browser reads, so the
 * attribute is what is checked, on every link the shell draws.
 */
test('every link carries the prefix, and a ctrl-click stays under it @subpath', async ({
  page,
  context,
  instanceId,
}) => {
  expect(instanceId).toBeTruthy();
  await page.goto(`${BASE}/`);
  await expect(page.locator('.page-title')).toBeVisible();

  const hrefs = await page
    .locator('a[href^="/"]')
    .evaluateAll((anchors) => anchors.map((a) => a.getAttribute('href') ?? ''));
  expect(hrefs.length).toBeGreaterThanOrEqual(13);
  for (const href of hrefs) {
    expect(href, `${href} leaves the mount point`).toMatch(new RegExp(`^${BASE}(/|$)`));
  }

  const [opened] = await Promise.all([
    context.waitForEvent('page'),
    page
      .locator('.sidebar-nav a')
      .nth(1)
      .click({ modifiers: ['Control'] }),
  ]);
  await opened.waitForLoadState();
  expect(new URL(opened.url()).pathname.startsWith(`${BASE}/`)).toBe(true);
  await expect(opened.locator('.page-title')).toBeVisible();
  await opened.close();
});

test('the API is reached through the prefix @subpath', async ({ page, instanceId }) => {
  expect(instanceId).toBeTruthy();
  const calls: string[] = [];
  page.on('request', (r) => {
    if (r.url().includes('/api/v1/')) calls.push(new URL(r.url()).pathname);
  });

  await page.goto(`${BASE}/instances`);
  await expect(page.locator('.page-title')).toBeVisible();

  expect(calls.length).toBeGreaterThan(0);
  for (const path of calls) {
    expect(path.startsWith(`${BASE}/api/v1/`), `${path} left the mount point`).toBe(true);
  }
});

test('the webhook URL it hands to Radarr carries the prefix @subpath', async ({
  page,
  instanceId,
}) => {
  expect(instanceId).toBeTruthy();
  await page.goto(`${BASE}/instances`);

  // Radarr calls this back; missing the prefix, every event would 404 at the
  // proxy and the user would see nothing at all.
  const url = await page.evaluate(async () => {
    // Reads the key where the application reads it, so this raw call is
    // authenticated exactly as the app's own requests are.
    const key = window.localStorage.getItem('routarr.apiKey') ?? '';
    const res = await fetch(`${document.baseURI}api/v1/instances`, {
      headers: key ? { 'x-api-key': key } : {},
    });
    const list = (await res.json()) as { webhook_url: string }[];
    return list[0]?.webhook_url;
  });

  expect(url).toMatch(new RegExp(`^${BASE}/api/v1/webhook/`));
});
