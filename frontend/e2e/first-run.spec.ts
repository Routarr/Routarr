import { test, expect } from './fixtures';

/**
 * Authentication is on by default: a key is generated at first start, so a
 * browser that has never been given one is the **ordinary** first visit, not an
 * error state.
 *
 * Mounting the shell behind it instead looks like a broken install: a failing
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
    await expect(page.getByRole('navigation', { name: 'Main navigation' })).toHaveCount(0);

    // And it says where to find the key, because "enter your API key" helps
    // nobody who has never seen one. Read with the real dictionary, so a
    // placeholder left unsubstituted shows here as `{file}` and fails.
    // The log line that printed the key goes with the container, and the file
    // on the volume is what survives, so the screen names it, the command that
    // reads it, and the variable for whoever would rather choose the key.
    await expect(page.getByText('saved it as routarr.api_key next to the database')).toBeVisible();
    await expect(
      page.getByText('docker exec routarr cat /config/routarr.api_key', { exact: true }),
    ).toBeVisible();
    await expect(page.getByText(/set ROUTARR_API_KEY and restart/)).toBeVisible();

    // Only the shell asks for anything. The pages are never mounted, so none of
    // them fires its own doomed request: a failure per page is what makes an
    // ordinary first visit look like a broken install.
    const pageData = refused.filter((path) =>
      /\/(media|rules|instances|decisions|overrides|jobs|logs|root-folders|backups)/.test(path),
    );
    expect(pageData, `pages fetched behind the gate: ${pageData.join(', ')}`).toHaveLength(0);

    await context.close();
  });

  test('is told when the key it typed is the wrong one', async ({ browser }) => {
    const context = await browser.newContext();
    const page = await context.newPage();
    await page.goto('/');

    // Said plainly, or the user pastes the same bad key again.
    await page.getByLabel('Routarr API key').fill('not-the-right-key');
    await page.getByRole('button', { name: 'Sign in' }).click();

    await expect(page.getByText('That key was refused. Check it and try again.')).toBeVisible();
    await expect(page.getByText('This Routarr needs an API key')).toBeVisible();

    await context.close();
  });

  /**
   * The key opens a session and the browser keeps no copy of it: held in the
   * page's storage, any script that ever ran there could read a key that opens
   * everything.
   */
  test('reaches the application once the key is given, and keeps no copy', async ({ browser }) => {
    const context = await browser.newContext();
    const page = await context.newPage();
    await page.goto('/');

    await page.getByLabel('Routarr API key').fill(process.env.ROUTARR_E2E_KEY ?? '');
    await page.getByRole('button', { name: 'Sign in' }).click();

    // The shell, for real: the navigation is back and the status bar answers.
    await expect(page.getByRole('navigation', { name: 'Main navigation' })).toBeVisible();
    await expect(page.getByRole('status', { name: 'Dry-run: writes blocked' })).toBeVisible();
    expect(await page.evaluate(() => JSON.stringify({ ...window.localStorage }))).not.toContain(
      process.env.ROUTARR_E2E_KEY ?? '',
    );

    await context.close();
  });

  /** A browser that stored the key before it was exchanged for a session. */
  test('trades a key it still holds for a session, once', async ({ browser }) => {
    const context = await browser.newContext();
    const page = await context.newPage();
    const key = process.env.ROUTARR_E2E_KEY ?? '';
    // Seeded on the first load only: an init script runs on every one, and the
    // reload below proves the session holds once the key is gone.
    await context.addInitScript((stored) => {
      if (!sessionStorage.getItem('seeded')) {
        sessionStorage.setItem('seeded', 'yes');
        localStorage.setItem('routarr.apiKey', stored);
      }
    }, key);
    await page.goto('/');

    await expect(page.getByRole('status', { name: 'Dry-run: writes blocked' })).toBeVisible();
    expect(await page.evaluate(() => localStorage.getItem('routarr.apiKey'))).toBeNull();

    await page.reload();
    await expect(page.getByRole('status', { name: 'Dry-run: writes blocked' })).toBeVisible();

    await context.close();
  });
});
