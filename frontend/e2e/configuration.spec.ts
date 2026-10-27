import { test, expect, openScreen, proveWithKey, API } from './fixtures';

/**
 * What only the whole stack in a browser shows of the configuration: a key
 * made on the screen opens the API and dies revoked, and a theme resolves to
 * different colours.
 */

test.describe('application keys', () => {
  /**
   * The token is read off the screen once and used as another application
   * would: it answers, and revoked it opens nothing. What each scope allows is
   * the server's to prove.
   */
  test('a key made on the screen opens the API and dies revoked', async ({ page }) => {
    await openScreen(page, '/applications');
    await page.getByRole('button', { name: 'New key' }).click();
    const dialog = page.getByRole('dialog');
    await dialog.getByLabel('Name').fill('n8n');
    await dialog.getByRole('button', { name: 'Create a key' }).click();
    await proveWithKey(page);

    // Read as the reader reads it, the one token on the page.
    const token = ((await page.getByText(/^rtr_\S+$/).textContent()) ?? '').trim();
    expect(token).toMatch(/^rtr_/);
    const status = () => fetch(`${API}/status`, { headers: { 'x-api-key': token } });

    expect((await status()).status).toBe(200);

    await page.getByRole('button', { name: 'Revoke – n8n' }).click();
    await page.getByRole('alertdialog').getByRole('button', { name: 'Revoke' }).click();
    await expect(page.getByText('No application has a key yet.')).toBeVisible();
    expect((await status()).status).toBe(401);
  });
});

test.describe('theme', () => {
  /**
   * The palette is a set of CSS custom properties, so "the light theme works"
   * means the browser actually resolved different values, which no unit test
   * can see. It also guards the split between the accent used as a *fill*
   * (unchanged, dark text on orange) and as *text*, which has to darken or it
   * falls short of AA contrast on white.
   */
  test('choosing the light theme repaints the interface', async ({ page }) => {
    const read = () =>
      page.evaluate(() => {
        const style = getComputedStyle(document.documentElement);
        return {
          background: style.getPropertyValue('--bg-base').trim(),
          text: style.getPropertyValue('--text-primary').trim(),
          accentFill: style.getPropertyValue('--accent-primary').trim(),
          accentText: style.getPropertyValue('--accent-strong').trim(),
          marker: document.documentElement.dataset.theme ?? 'auto',
        };
      });

    await page.goto('/settings#general');
    await expect(page.getByRole('heading', { level: 1 })).toBeVisible();
    const dark = await read();
    expect(dark.marker).toBe('dark');

    await page.getByLabel('Theme').selectOption('light');
    await page.getByRole('button', { name: 'Save', exact: true }).click();
    await expect(page.locator('.banner-success')).toBeVisible();

    await page.reload();
    await expect(page.getByRole('heading', { level: 1 })).toBeVisible();
    const light = await read();

    expect(light.marker).toBe('light');
    expect(light.background).not.toBe(dark.background);
    expect(light.text).not.toBe(dark.text);
    // The fill is shared between the two themes, and only the text form darkens.
    expect(light.accentFill).toBe(dark.accentFill);
    expect(light.accentText).not.toBe(dark.accentText);
  });
});
