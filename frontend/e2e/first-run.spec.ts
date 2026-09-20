import { test, expect } from './fixtures';

/**
 * Authentication is on by default: a key is generated at first start, so a
 * browser that has never been given one is the **ordinary** first visit, not an
 * error state.
 *
 * Mounting the shell behind it instead looks like a broken install — a failing
 * request per page, every page empty, and a small "Unauthorized" badge whose
 * remedy the user has to already know. These tests are the guard on that.
 */
test.describe('a browser with no API key', () => {
  test('is asked for one instead of being shown a broken application', async ({ browser }) => {
    const context = await browser.newContext();
    const page = await context.newPage();

    const refused: string[] = [];
    page.on('response', (response) => {
      if (response.status() === 401) refused.push(new URL(response.url()).pathname);
    });

    await page.goto('/');

    // The prompt, not the shell.
    await expect(page.getByText('This Routarr needs an API key')).toBeVisible();
    await expect(page.locator('.sidebar-nav')).toHaveCount(0);

    // And it says where to find the key, because "enter your API key" helps
    // nobody who has never seen one — with the real dictionary, so a placeholder
    // left unsubstituted shows here as `{file}` and fails.
    // The log line that printed the key goes with the container; the file on
    // the volume is what survives, so the screen names it, the command that
    // reads it, and the variable for whoever would rather choose the key.
    await expect(page.getByText('saved it as routarr.api_key next to the database')).toBeVisible();
    await expect(
      page.getByText('docker exec routarr cat /data/routarr.api_key', { exact: true }),
    ).toBeVisible();
    await expect(page.getByText(/set ROUTARR_API_KEY and restart/)).toBeVisible();

    // Only the shell asks for anything. The twelve pages are never mounted, so
    // none of them fires its own doomed request — a failure per page is what
    // makes an ordinary first visit look like a broken install.
    const pageData = refused.filter((path) =>
      /\/(media|rules|instances|decisions|overrides|jobs|logs|root-folders|backups)/.test(path),
    );
    expect(pageData, `pages fetched behind the gate: ${pageData.join(', ')}`).toHaveLength(0);

    await context.close();
  });

  test('is told when the key it has is the wrong one', async ({ browser }) => {
    const context = await browser.newContext();
    const page = await context.newPage();

    // A key already stored and still refused is a *wrong* key, not a missing
    // one. Said plainly, or the user pastes the same bad key again.
    await context.addInitScript(() =>
      window.localStorage.setItem('routarr.apiKey', 'not-the-right-key'),
    );
    await page.goto('/');

    await expect(page.getByText('That key was refused. Check it and try again.')).toBeVisible();
    await expect(page.getByText('This Routarr needs an API key')).toBeVisible();

    await context.close();
  });

  test('reaches the application once the key is given', async ({ browser }) => {
    const context = await browser.newContext();
    const page = await context.newPage();
    await page.goto('/');

    await page.locator('#gate-api-key').fill(process.env.ROUTARR_E2E_KEY ?? '');
    await page.getByRole('button', { name: /save key/i }).click();

    // The shell, for real: the navigation is back and the status bar answers.
    await expect(page.locator('.sidebar-nav')).toBeVisible();
    await expect(page.locator('.topbar .mode-chip')).toBeVisible();

    await context.close();
  });
});
