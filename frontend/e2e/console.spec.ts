import { test, expect } from './fixtures';
import { SCREENS } from './screens';

/**
 * Nothing in normal use should reach the console.
 *
 * A framework warning, a failed request, a thrown promise: each is invisible in
 * a screenshot and each is a real defect. Asserting on silence is the cheapest
 * way to notice one the day it appears, rather than the day a user reports the
 * symptom it eventually causes.
 */
test('no page writes an error or a warning to the console @console', async ({
  page,
  instanceId,
}) => {
  expect(instanceId).toBeTruthy();
  const noise: string[] = [];

  page.on('console', (message) => {
    if (message.type() === 'error' || message.type() === 'warning') {
      noise.push(`[${message.type()}] ${message.text()}`);
    }
  });
  page.on('pageerror', (error) => noise.push(`[pageerror] ${error.message}`));
  page.on('requestfailed', (request) =>
    noise.push(`[requestfailed] ${request.url()} — ${request.failure()?.errorText}`),
  );
  page.on('response', (response) => {
    if (response.status() >= 400) {
      noise.push(`[http ${response.status()}] ${new URL(response.url()).pathname}`);
    }
  });

  for (const path of SCREENS) {
    await page.goto(path);
    await expect(page.locator('.page-title')).toBeVisible();
  }

  expect(noise, `the console was not silent:\n${[...new Set(noise)].join('\n')}`).toHaveLength(0);
});
