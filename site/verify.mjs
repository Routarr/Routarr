#!/usr/bin/env node
/**
 * Browser verification of the showcase site.
 *
 * Runs against `serve.mjs`, which applies the real `_headers`, so the strict
 * Content-Security-Policy is exercised rather than admired. A CSP that blocks
 * the stylesheet looks fine in the file and blank in the browser.
 *
 *   node site/verify.mjs
 */
import { spawn } from 'node:child_process';
import { createRequire } from 'node:module';
import { fileURLToPath } from 'node:url';

const ROOT = fileURLToPath(new URL('.', import.meta.url));
const require = createRequire(`${process.env.FRONTEND_DIR ?? `${ROOT}../frontend`}/package.json`);
const { chromium } = require('@playwright/test');
const { AxeBuilder } = require('@axe-core/playwright');

/**
 * A port nothing else holds: a fixed one collided with a second checkout
 * verifying at the same time, and the failure read as a broken page.
 */
async function freePort() {
  const { createServer } = await import('node:net');
  return new Promise((resolve, reject) => {
    const probe = createServer();
    probe.once('error', reject);
    probe.listen(0, '127.0.0.1', () => {
      const { port } = probe.address();
      probe.close(() => resolve(port));
    });
  });
}

const PORT = await freePort();
const BASE = `http://127.0.0.1:${PORT}`;

const failures = [];
const fail = (message) => failures.push(message);
const check = (condition, message) => { if (!condition) fail(message); };

const server = spawn(process.execPath, [`${ROOT}serve.mjs`, String(PORT)], { stdio: 'ignore' });
const stop = () => server.kill();
process.on('exit', stop);

async function waitForServer() {
  for (let i = 0; i < 60; i += 1) {
    try {
      if ((await fetch(`${BASE}/`)).ok) return;
    } catch {
      /* not up yet */
    }
    await new Promise((r) => setTimeout(r, 100));
  }
  throw new Error('the preview server did not start');
}

await waitForServer();

const browser = await chromium.launch();
const context = await browser.newContext({ viewport: { width: 1440, height: 900 } });
const page = await context.newPage();

const problems = [];
page.on('console', (message) => {
  if (message.type() === 'error') problems.push(`console: ${message.text()}`);
});
page.on('pageerror', (error) => problems.push(`uncaught: ${error.message}`));
page.on('requestfailed', (request) => problems.push(`failed request: ${request.url()}`));

const hosts = new Set();
page.on('request', (request) => hosts.add(new URL(request.url()).host));

await page.addInitScript(() => {
  window.__csp = [];
  document.addEventListener('securitypolicyviolation', (e) => {
    window.__csp.push(`${e.violatedDirective} blocked ${e.blockedURI}`);
  });
});

await page.goto(`${BASE}/`, { waitUntil: 'networkidle' });

// ------------------------------------------------------------ clean load
check(problems.length === 0, `the page reports problems:\n      ${problems.join('\n      ')}`);
const violations = await page.evaluate(() => window.__csp);
check(violations.length === 0, `content-security-policy violations:\n      ${violations.join('\n      ')}`);

// ------------------------------------------------------------ no third party
const external = [...hosts].filter((h) => h !== `127.0.0.1:${PORT}`);
check(external.length === 0, `the page contacts external hosts: ${external.join(', ')}`);

// ------------------------------------------------------------ images
// Force every image to load rather than relying on the scroll position to
// trigger `loading="lazy"`: the point here is that each URL resolves and
// decodes, and depending on lazy heuristics made the check intermittent.
await page.evaluate(() => {
  for (const image of document.images) image.loading = 'eager';
});
await page.waitForFunction(() => [...document.images].every((i) => i.complete), null, {
  timeout: 20_000,
});

const images = await page.evaluate(() =>
  [...document.images].map((i) => ({ src: i.currentSrc || i.src, ok: i.naturalWidth > 0 })),
);
// No assertion that the page carries an image: it carries none since the
// screenshots came out of it. The loop below is the one that matters, and it
// holds whatever the count.
for (const image of images) check(image.ok, `image did not load: ${image.src}`);

// ----------------------------------------------------------- accessibility
// Every page at WCAG 2.1 AA, at a desktop and a phone width: what the
// application's own sweep holds itself to, held here as well.
/* The eight content pages: four languages of the landing, four of the detail
   page. Named once, because a probe that quietly stops covering half the site
   is the kind that keeps passing. */
const LANDINGS = ['/', '/fr/', '/de/', '/es/'];
const DETAILS = ['/how/', '/fr/how/', '/de/how/', '/es/how/'];
const PAGES = [...LANDINGS, ...DETAILS];

for (const path of [...PAGES, '/404.html']) {
  for (const width of [1440, 375]) {
    const tab = await context.newPage();
    await tab.setViewportSize({ width, height: 900 });
    await tab.goto(`${BASE}${path}`, { waitUntil: 'networkidle' });
    const { violations } = await new AxeBuilder({ page: tab })
      .withTags(['wcag2a', 'wcag2aa', 'wcag21a', 'wcag21aa'])
      .analyze();
    for (const violation of violations) {
      const where = violation.nodes.map((node) => node.target.join(' ')).slice(0, 3).join(', ');
      fail(`${path} at ${width}px: ${violation.id} (${violation.impact}), ${where}`);
    }
    await tab.close();
  }
}

// ------------------------------------------------------------ layout
// All four pages, because a German phrase in a fixed column is exactly the
// kind of thing that only overflows on one of them, and a failure names the
// element: "the page scrolls sideways by 40px" is a fact nobody can act on.
for (const path of PAGES) {
  const tab = await context.newPage();
  await tab.goto(`${BASE}${path}`, { waitUntil: 'networkidle' });
  for (const width of [360, 390, 480, 560, 600, 768, 900, 1024, 1280, 1440]) {
    await tab.setViewportSize({ width, height: 900 });
    await tab.waitForTimeout(90);
    const { overflow, culprits } = await tab.evaluate(() => {
      const root = document.documentElement;
      const over = root.scrollWidth - root.clientWidth;
      if (over <= 1) return { overflow: over, culprits: [] };
      // The widest offenders, not every descendant of one: a block that spills
      // takes all its children with it and the list becomes unreadable.
      const spilling = [...document.querySelectorAll('body *')]
        .map((el) => ({ el, right: el.getBoundingClientRect().right }))
        .filter(({ el, right }) => right > root.clientWidth + 1 && getComputedStyle(el).position !== 'fixed')
        .filter(({ el }) => !el.parentElement || el.parentElement.getBoundingClientRect().right <= root.clientWidth + 1)
        .map(({ el, right }) => `${el.tagName.toLowerCase()}.${el.className || '(none)'} by ${Math.round(right - root.clientWidth)}px`);
      return { overflow: over, culprits: [...new Set(spilling)].slice(0, 4) };
    });
    check(
      overflow <= 1,
      `${path} at ${width}px scrolls sideways by ${overflow}px: ${culprits.join(' | ') || 'no single element found'}`,
    );
  }
  await tab.close();
}
await page.setViewportSize({ width: 1440, height: 900 });

// ------------------------------------------------------------ header
// The bar holds the brand, the two switches, the Index control and the
// repository on one row until the row is told to wrap, and the switches are as
// wide as the translated words make them. A band of widths where they run
// under the Index control is invisible to the overflow check above, since
// nothing scrolls. No two controls may share a pixel, on any page, at any of
// these widths — and the panel is measured open as well, where it overlays the
// page and must still clear the bar that opened it.
let headerControls = 0;
for (const path of PAGES) {
  const tab = await context.newPage();
  await tab.goto(`${BASE}${path}`, { waitUntil: 'networkidle' });
  for (const width of [360, 600, 739, 768, 860, 900, 1024, 1100, 1440]) {
    await tab.setViewportSize({ width, height: 900 });
    await tab.waitForTimeout(80);
    for (const open of [false, true]) {
      const { count, overlaps } = await tab.evaluate((wantOpen) => {
        const index = document.querySelector('.index');
        if (index) index.open = wantOpen;
        // `checkVisibility`, not `offsetParent`: Chromium hides a closed
        // <details>' content with `content-visibility`, which leaves both an
        // offset parent and the rect it last had, so the panel's six links
        // read as laid over the bar that had just closed them.
        const boxes = [...document.querySelectorAll('.site-header a, .site-header button, .site-header summary')]
          .filter((el) => (el.checkVisibility ? el.checkVisibility() : el.offsetParent !== null))
          .map((el) => ({
            name: el.textContent.trim() || el.getAttribute('aria-label') || el.tagName,
            box: el.getBoundingClientRect(),
          }));
        const overlaps = [];
        for (let i = 0; i < boxes.length; i += 1) {
          for (let j = i + 1; j < boxes.length; j += 1) {
            const a = boxes[i].box;
            const b = boxes[j].box;
            const x = Math.min(a.right, b.right) - Math.max(a.left, b.left);
            const y = Math.min(a.bottom, b.bottom) - Math.max(a.top, b.top);
            if (x > 1 && y > 1) overlaps.push(`"${boxes[i].name}" over "${boxes[j].name}" by ${Math.round(x)}px`);
          }
        }
        return { count: boxes.length, overlaps };
      }, open);
      headerControls += count;
      const state = open ? 'index open' : 'index closed';
      for (const overlap of overlaps) fail(`${path} at ${width}px, ${state}: header controls overlap: ${overlap}`);
    }
  }
  await tab.close();
}
// Three controls on the bar and thirteen with the panel open on a landing,
// sixteen on the detail page, over nine widths and eight pages (1152 measured): a selector
// that stops matching would compare nothing and pass.
check(headerControls >= 1100, `measured ${headerControls} header control(s), expected at least 1100`);

// ------------------------------------------------------------ board
// The departure board sets its words in fixed flap cells and its status in a
// fixed column, while both are translated and the flap size changes with the
// width: a status one word longer in German, or a flap size wrong at one
// width, overflows its column and nothing else notices, since the board clips
// it. Every flap word, status and rule is measured against the box it sits in,
// on the four landing pages, in the three layouts. The board is the hero, so
// it is on those and nowhere else.
let boardWords = 0;
for (const path of LANDINGS) {
  const tab = await context.newPage();
  for (const width of [1280, 900, 390]) {
    await tab.setViewportSize({ width, height: 900 });
    await tab.goto(`${BASE}${path}`, { waitUntil: 'networkidle' });
    const measured = await tab.evaluate(() => {
      const board = document.querySelector('.board');
      if (!board) return null;
      const edge = board.getBoundingClientRect().right - 1;
      return [...board.querySelectorAll('.flaps, .st, .dep-rule')]
        .filter((el) => el.offsetParent !== null)
        .map((el) => {
          const own = el.getBoundingClientRect();
          const cell = el.closest('td, th');
          const clipped = cell ? Math.max(0, el.scrollWidth - cell.clientWidth) : 0;
          return {
            word: el.textContent.trim().slice(0, 24),
            spill: Math.round(Math.max(own.right - edge, clipped)),
          };
        });
    });
    if (!measured) {
      fail(`${path} at ${width}px: the departure board is missing`);
      continue;
    }
    for (const { word, spill } of measured) {
      boardWords += 1;
      if (spill > 0) fail(`${path} at ${width}px: "${word}" overflows its column by ${spill}px`);
    }
  }
  await tab.close();
}
// The count is the guard on the guard: a selector that stops matching would
// measure nothing and pass.
check(boardWords >= 120, `measured ${boardWords} board word(s) across four pages, expected at least 120`);

// ------------------------------------------------------------ anchors
const anchors = await page.evaluate(() =>
  [...document.querySelectorAll('a[href^="#"]')]
    .map((a) => a.getAttribute('href').slice(1))
    .filter((id) => id && !document.getElementById(id)),
);
check(anchors.length === 0, `in-page links pointing nowhere: ${anchors.join(', ')}`);

// ------------------------------------------------------------ theme
// Both states are named, so the test asks for the one the page is not in:
// clicking the lit cell is a no-op by design and would report a dead control.
const before = await page.evaluate(() => getComputedStyle(document.body).backgroundColor);
const other = await page.evaluate(() =>
  document.documentElement.dataset.theme === 'light' ? 'dark' : 'light',
);
// The switch lives inside the Index panel now, so it has to be opened first.
// Both settings moved off the bar: eight controls beside the brand wrapped it
// onto a second row on a phone.
await page.click('.index > summary');
await page.click(`[data-theme-set="${other}"]`);
await page.waitForTimeout(120);
const after = await page.evaluate(() => getComputedStyle(document.body).backgroundColor);
check(before !== after, 'the theme switch changed nothing');
check(
  await page.evaluate((want) => document.querySelector(`[data-theme-set="${want}"]`).getAttribute('aria-pressed') === 'true', other),
  'the theme switch does not say which state it is in',
);

const chosen = await page.evaluate(() => document.documentElement.dataset.theme);
await page.reload({ waitUntil: 'networkidle' });
const persisted = await page.evaluate(() => document.documentElement.dataset.theme);
check(persisted === chosen, `the theme choice did not survive a reload (${chosen} became ${persisted})`);

// ------------------------------------------------------------ accessibility
const unnamed = await page.evaluate(() =>
  [...document.querySelectorAll('button, a')]
    .filter((el) => !el.textContent.trim() && !el.getAttribute('aria-label') && !el.title)
    .map((el) => el.outerHTML.slice(0, 70)),
);
check(unnamed.length === 0, `controls with no accessible name:\n      ${unnamed.join('\n      ')}`);

await page.keyboard.press('Tab');
const firstStop = await page.evaluate(() => document.activeElement?.className ?? '');
check(firstStop.includes('skip'), `the first tab stop should be the skip link, got "${firstStop}"`);

// ------------------------------------------------------------ 404
const missing = await page.goto(`${BASE}/does-not-exist`);
check(missing.status() === 404, `an unknown path answered ${missing.status()}, expected 404`);
check(
  (await page.locator('h1').count()) === 1,
  'the 404 page does not have exactly one heading',
);

await browser.close();
stop();

if (failures.length) {
  console.error(`\n${failures.length} problem(s):\n`);
  for (const problem of failures) console.error(`  - ${problem}`);
  process.exit(1);
}
console.log('site verified in a browser');
