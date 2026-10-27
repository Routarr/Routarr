import { test, expect, openScreen, screenShown } from './fixtures';
import { SCREENS } from './screens';

/**
 * Every screen, reached by its sidebar link as a reader reaches it: a link that loads no page, a
 * load that fails, an alert, or a line in the console fails here.
 */
test('every sidebar entry reaches a screen that loads in silence', async ({ page }) => {
  const noise: string[] = [];
  // Counted by hand: a sidebar link loads no page, so networkidle answers before the loads.
  let inFlight = 0;

  page.on('console', (message) => {
    if (message.type() === 'error' || message.type() === 'warning') {
      noise.push(`[${message.type()}] ${message.text()}`);
    }
  });
  page.on('pageerror', (error) => noise.push(`[pageerror] ${error.message}`));
  page.on('request', () => (inFlight += 1));
  page.on('requestfinished', () => (inFlight -= 1));
  page.on('requestfailed', (request) => {
    inFlight -= 1;
    noise.push(`[requestfailed] ${request.url()} – ${request.failure()?.errorText}`);
  });
  page.on('response', (response) => {
    if (response.status() >= 400) {
      noise.push(`[http ${response.status()}] ${new URL(response.url()).pathname}`);
    }
  });

  await openScreen(page, '/');
  // Read from the sidebar, so a link added without a page fails this test, and a screen left out.
  const links = page.getByRole('navigation', { name: 'Main navigation' }).getByRole('link');
  await expect(links).toHaveCount(SCREENS.length);

  for (let index = 0; index < SCREENS.length; index += 1) {
    const link = links.nth(index);
    const label = (await link.innerText()).trim();
    await link.click();

    await expect(link, `${label} is not where its link went`).toHaveAttribute(
      'aria-current',
      'page',
    );
    await screenShown(page, label);
    // The heading draws before the loads, and a load answering 500 shows its banner after.
    await expect.poll(() => inFlight, { message: `${label} never settles` }).toBe(0);
    await expect(page.getByRole('alert'), `${label} shows an error`).toHaveCount(0);
  }

  expect(noise, `the console was not silent:\n${[...new Set(noise)].join('\n')}`).toHaveLength(0);
});
