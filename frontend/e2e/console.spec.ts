import { test, expect, openScreen } from './fixtures';
import { SCREENS } from './screens';

test('no page writes an error or a warning to the console @console', async ({ page }) => {
  const noise: string[] = [];

  page.on('console', (message) => {
    if (message.type() === 'error' || message.type() === 'warning') {
      noise.push(`[${message.type()}] ${message.text()}`);
    }
  });
  page.on('pageerror', (error) => noise.push(`[pageerror] ${error.message}`));
  page.on('requestfailed', (request) =>
    noise.push(`[requestfailed] ${request.url()} – ${request.failure()?.errorText}`),
  );
  page.on('response', (response) => {
    if (response.status() >= 400) {
      noise.push(`[http ${response.status()}] ${new URL(response.url()).pathname}`);
    }
  });

  // The heading draws before the screen's requests answer, so wait for them too.
  for (const path of SCREENS) {
    await openScreen(page, path);
    await page.waitForLoadState('networkidle');
  }

  expect(noise, `the console was not silent:\n${[...new Set(noise)].join('\n')}`).toHaveLength(0);
});
