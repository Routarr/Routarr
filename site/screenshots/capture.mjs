/**
 * Capture the README's screenshot of the simulation from the real application.
 *
 * A mockup ages into a lie the moment the interface moves. This one comes out
 * of the same binary the e2e suite drives, against a throwaway database, so a
 * screen that no longer looks like this is a screen that changed.
 */
import { readFileSync } from 'node:fs';

import { chromium } from '../playwright.mjs';

const BASE = process.env.ROUTARR_URL ?? 'http://127.0.0.1:9899';
const OUT = process.env.SHOT_PNG ?? new URL('./simulation.png', import.meta.url).pathname;
// A renamed route opens the not-found screen, which the shutter would take
// without a word.
const NOT_FOUND = JSON.parse(
  readFileSync(new URL('../../backend/locales/en.json', import.meta.url), 'utf-8'),
).NotFoundTitle;

const browser = await chromium.launch();
const context = await browser.newContext({
  viewport: { width: 1440, height: 900 },
  deviceScaleFactor: 2,
  colorScheme: 'dark',
});

// The interface reads its key from localStorage. Without it the dashboard shows
// the "API is unauthenticated" warning, which is correct behaviour but not what
// a configured installation looks like.
if (process.env.ROUTARR_API_KEY) {
  await context.addInitScript(
    (key) => window.localStorage.setItem('routarr.apiKey', key),
    process.env.ROUTARR_API_KEY,
  );
}

const page = await context.newPage();
await page.goto(`${BASE}/simulation`);
await page.waitForLoadState('networkidle');
const heading = (await page.locator('h1').first().textContent())?.trim();
if (!heading || heading === NOT_FOUND) throw new Error('/simulation opened the not-found screen');

const run = page.getByRole('button', { name: /run simulation/i });
if (await run.count()) {
  await run.first().click();
  await page.waitForTimeout(1500);
  await page.waitForLoadState('networkidle');
}
// The decisions are what the README describes: their table is scrolled into
// view, past the totals and the destinations' capacity.
await page.locator('table').last().evaluate((table) => table.closest('section, .card')?.scrollIntoView());
// Let the dark theme's transitions land before the shutter.
await page.waitForTimeout(400);
await page.screenshot({ path: OUT });
console.log(`captured ${OUT}`);

await browser.close();
