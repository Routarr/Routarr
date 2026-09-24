import { test, expect } from './fixtures';

/**
 * The provider sign-in, in a browser.
 *
 * The harness serves in `apikey` mode, so the test answers the mode the shell
 * asks for and stands in for the provider with a page of its own: what is
 * under test is that the link leaves the application. The router listens to
 * every click on the document, and a click it takes changes the address and
 * loads nothing, so the proof is the provider's page on screen, not the
 * address.
 */
for (const [base, tag] of [
  ['', ''],
  ['/routarr', ' @subpath'],
] as const) {
  test(`the provider link leaves the application${tag}`, async ({ browser }) => {
    const context = await browser.newContext();
    const page = await context.newPage();
    await page.route('**/api/v1/auth/mode', (route) =>
      route.fulfill({ json: { mode: 'oidc', api_key_configured: false, api_key_pinned: false } }),
    );
    await page.route('**/api/v1/auth/oidc/start', (route) =>
      route.fulfill({ contentType: 'text/html', body: '<h1>Identity provider</h1>' }),
    );

    await page.goto(`${base}/`);
    await page.getByRole('link', { name: 'Sign in with your provider' }).click();

    await expect(page.getByRole('heading', { name: 'Identity provider' })).toBeVisible();
    await context.close();
  });
}
