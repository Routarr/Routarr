/**
 * Capture the showcase screenshots from the real application.
 *
 * Mockups age into lies the moment the interface moves. These come out of the
 * same binary the E2E suite drives, against a throwaway database, so a screen
 * that no longer looks like this is a screen that changed.
 */
import { createRequire } from 'node:module';
import { mkdirSync } from 'node:fs';

// Resolved against the frontend, which owns the dependency: ESM resolves from
// the *file's* directory, so a plain import would look under site/ and fail
// however the script is launched.
const require = createRequire(`${process.env.FRONTEND_DIR}/package.json`);
const { chromium } = require('@playwright/test');

const BASE = process.env.ROUTARR_URL ?? 'http://127.0.0.1:9899';
const OUT = process.env.SHOTS_DIR ?? new URL('../public/assets/shots/', import.meta.url).pathname;

mkdirSync(OUT, { recursive: true });

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

async function settle() {
  await page.waitForLoadState('networkidle');
  // Let the dark theme's transitions land before the shutter.
  await page.waitForTimeout(400);
}

async function shot(name) {
  await page.screenshot({ path: `${OUT}/${name}.png` });
  console.log(`captured ${name}`);
}

// --- dashboard ------------------------------------------------------------
await page.goto(`${BASE}/`);
await settle();
await shot('dashboard');

// --- root folders ---------------------------------------------------------
await page.goto(`${BASE}/root-folders`);
await settle();
await shot('root-folders');

// --- simulation -----------------------------------------------------------
await page.goto(`${BASE}/simulation`);
await settle();
const run = page.getByRole('button', { name: /run simulation/i });
if (await run.count()) {
  await run.first().click();
  await page.waitForTimeout(1500);
  await settle();
}
await shot('simulation');

// --- the explanation panel ------------------------------------------------
// The differentiator: condition by condition, expected against observed.
await page.goto(`${BASE}/media`);
await settle();
const why = page.getByRole('button', { name: /why/i });
if (await why.count()) {
  // Taller viewport so the whole panel is on screen, then frame the panel
  // itself: at the size the site displays it, a full-window shot of a dense
  // modal is unreadable, which defeats the point of showing it.
  await page.setViewportSize({ width: 1440, height: 1500 });
  await why.first().click();
  const panel = page.locator('.modal-content');
  await panel.waitFor({ timeout: 10_000 });
  await page.waitForTimeout(600);
  await panel.screenshot({ path: `${OUT}/explain.png` });
  console.log('captured explain');
  await page.setViewportSize({ width: 1440, height: 900 });
} else {
  // The differentiator missing from the site is not a shot to skip: the run
  // ends here, and the pair on disk stays what it was.
  throw new Error('no "Why?" button found — the library has no decision to explain');
}

// --- the Open Graph card -------------------------------------------------
// Rendered in the same browser, so it uses real text layout rather than
// ImageMagick's SVG renderer, which has no usable fonts here.
const card = await browser.newPage({ viewport: { width: 1200, height: 630 }, deviceScaleFactor: 1 });
await card.goto(new URL('./og.html', import.meta.url).href);
await card.waitForTimeout(300);
await card.screenshot({ path: `${OUT}/../og.png` });
console.log('captured og');

await browser.close();
