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
import { fileURLToPath } from 'node:url';

import { chromium, fromFrontend } from './playwright.mjs';

const ROOT = fileURLToPath(new URL('.', import.meta.url));
const { AxeBuilder } = fromFrontend('@axe-core/playwright');

/**
 * A port nothing else holds: a fixed one collides with a second checkout
 * verifying at the same time, and the failure reads as a broken page.
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

// On the context, not on the first page: every page the sweeps below open is
// watched as well, so a component only `/how/` renders cannot break the CSP or
// throw with every gate green.
const problems = [];
context.on('console', (message) => {
  if (message.type() === 'error') problems.push(`console: ${message.text()}`);
});
context.on('weberror', (error) => problems.push(`uncaught: ${error.error().message}`));
context.on('requestfailed', (request) => problems.push(`failed request: ${request.url()}`));

const hosts = new Set();
context.on('request', (request) => hosts.add(new URL(request.url()).host));

await context.addInitScript(() => {
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
// decodes, and lazy heuristics make the check intermittent.
await page.evaluate(() => {
  for (const image of document.images) image.loading = 'eager';
});
await page.waitForFunction(() => [...document.images].every((i) => i.complete), null, {
  timeout: 20_000,
});

const images = await page.evaluate(() =>
  [...document.images].map((i) => ({ src: i.currentSrc || i.src, ok: i.naturalWidth > 0 })),
);
// No assertion that the page carries an image: it may carry none. The loop
// below is the one that matters, and it holds whatever the count.
for (const image of images) check(image.ok, `image did not load: ${image.src}`);

// ----------------------------------------------------------- accessibility
// Every page at WCAG 2.1 AA, at a desktop and a phone width: what the
// application's own sweep holds itself to, held here as well.
/* The content pages: every language of the landing and of the detail page,
   from the one list the pages are built from, because a probe that quietly
   stops covering half the site is the kind that keeps passing. */
const { LANGUAGES } = await import('./src/i18n/languages.ts');
const LANDINGS = LANGUAGES.map(({ path }) => path);
const DETAILS = LANDINGS.map((path) => `${path}how/`);
const APIS = LANDINGS.map((path) => `${path}api/`);
const PAGES = [...LANDINGS, ...DETAILS, ...APIS];

const NOT_FOUND = LANDINGS.map((path) => `${path}404.html`);
for (const path of [...PAGES, ...NOT_FOUND]) {
  for (const width of [1440, 375]) {
    const tab = await context.newPage();
    await tab.setViewportSize({ width, height: 900 });
    const before = problems.length;
    await tab.goto(`${BASE}${path}`, { waitUntil: 'networkidle' });
    const blocked = await tab.evaluate(() => window.__csp);
    for (const violation of blocked) fail(`${path} at ${width}px: content-security-policy ${violation}`);
    for (const problem of problems.slice(before)) fail(`${path} at ${width}px: ${problem}`);
    const { violations, incomplete } = await new AxeBuilder({ page: tab })
      .withTags(['wcag2a', 'wcag2aa', 'wcag21a', 'wcag21aa'])
      .analyze();
    for (const violation of violations) {
      const where = violation.nodes.map((node) => node.target.join(' ')).slice(0, 3).join(', ');
      fail(`${path} at ${width}px: ${violation.id} (${violation.impact}), ${where}`);
    }
    // A contrast axe could not measure is a contrast nobody checked: text over
    // a layer it cannot see through reads as "incomplete", never as a failure.
    // Two exceptions: a glyph that is no text (`nonBmp`), a decorative mark,
    // and the hero's text over its grid (`pseudoContent`), which `check.mjs`
    // measures on the tinted ground instead.
    const reasons = (node) => node.any.map((check) => check.data?.messageKey);
    const candidates = incomplete
      .filter((result) => result.id === 'color-contrast')
      .flatMap((result) => result.nodes)
      .filter((node) => !reasons(node).every((key) => key === 'nonBmp'));
    const inHero = await tab.evaluate(
      (selectors) => selectors.map((selector) => Boolean(document.querySelector(selector)?.closest('.hero'))),
      candidates.map((node) => node.target.join(' ')),
    );
    const unmeasured = candidates.filter(
      (node, at) => !(inHero[at] && reasons(node).every((key) => key === 'pseudoContent')),
    );
    if (unmeasured.length) {
      const where = unmeasured.map((node) => node.target.join(' ')).slice(0, 3).join(', ');
      fail(`${path} at ${width}px: a contrast axe could not measure, ${where}`);
    }
    await tab.close();
  }
}

// ------------------------------------------------------------ layout
// Every page, because a German phrase in a fixed column is exactly the kind
// of thing that only overflows on one of them, and a failure names the
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

// ------------------------------------------------------------ clipping
// A box that holds its content by a fixed height, or shrinks under it behind
// `overflow: hidden`, loses words without scrolling the page, so the check
// above passes. Down to 320px (the width WCAG reflow names), every box is
// measured against its own content, with the copy button showing its longest
// label. Scroll regions scroll by design, an ellipsis cuts on purpose, and a
// glyph may reach past a tight line box by a few pixels.
let boxesMeasured = 0;
for (const path of PAGES) {
  const tab = await context.newPage();
  await tab.goto(`${BASE}${path}`, { waitUntil: 'networkidle' });
  await tab.evaluate(() => document.querySelectorAll('.copy-btn').forEach((button) => {
    button.textContent = button.dataset.failed;
  }));
  for (const width of [320, 360, 390]) {
    await tab.setViewportSize({ width, height: 900 });
    await tab.waitForTimeout(80);
    const { count, clipped } = await tab.evaluate(() => {
      const clipped = [];
      let count = 0;
      for (const el of document.querySelectorAll('main *')) {
        // Hidden for the eye and kept for a screen reader: `.sr-only`, or the
        // same clip a layout applies at one width only.
        let hidden = false;
        for (let up = el; up && !hidden; up = up.parentElement) {
          hidden = up.classList.contains('sr-only') || getComputedStyle(up).clipPath === 'inset(50%)';
        }
        if (!el.checkVisibility() || hidden) continue;
        const style = getComputedStyle(el);
        if (style.display === 'inline' || style.display === 'contents') continue;
        if (/auto|scroll/.test(`${style.overflowX} ${style.overflowY}`) || style.textOverflow === 'ellipsis') continue;
        count += 1;
        const hides = style.overflowX !== 'visible' || style.overflowY !== 'visible';
        const across = el.scrollWidth - el.clientWidth;
        const down = el.scrollHeight - el.clientHeight;
        if (across > 1 || down > (hides ? 1 : 4)) {
          const name = `${el.tagName.toLowerCase()}${[...el.classList].map((c) => `.${c}`).join('')}`;
          clipped.push(`${name} "${el.textContent.trim().slice(0, 30)}" by ${across}x${down}px`);
        }
      }
      return { count, clipped };
    });
    boxesMeasured += count;
    for (const box of clipped) fail(`${path} at ${width}px: ${box} beyond its box`);
  }
  await tab.close();
}
check(boxesMeasured >= 6500, `measured ${boxesMeasured} box(es) for clipping, expected at least 6500`);

// ------------------------------------------------------------ header
// The bar holds the brand, the Index control and the repository on one row,
// and the switches in the panel are as wide as the translated words make
// them. A band of widths where two controls overlap is invisible to the
// overflow check above, since nothing scrolls. No two controls may share a
// pixel, on any page, at any of these widths, and the panel is measured open
// as well, where it overlays the page and must still clear the bar that
// opened it.
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
// Every header control, closed and open, over nine widths and every page: a
// selector that stops matching would compare nothing and pass.
check(headerControls >= 1650, `measured ${headerControls} header control(s), expected at least 1650`);

// ------------------------------------------------------------- plan
// The hero's plan is a table of translated strings in columns that do not
// wrap: a rule name one word longer in German, or a longer status, pushes its
// cell past the card and nothing else notices. Every cell and every chip is
// measured against its own box, on the four landing pages, in the three
// layouts, and any text it clips fails. The plan is the hero, so it is on those
// and nowhere else.
let planCells = 0;
for (const path of LANDINGS) {
  const tab = await context.newPage();
  for (const width of [1280, 900, 390]) {
    await tab.setViewportSize({ width, height: 900 });
    await tab.goto(`${BASE}${path}`, { waitUntil: 'networkidle' });
    const measured = await tab.evaluate(() => {
      const plan = document.querySelector('.plan');
      if (!plan) return null;
      // Only clipping counts. The table scrolls sideways on a phone, so a cell
      // sitting past the visible frame is the scroller working, not a fault.
      // Measured against that edge, every row on every narrow page would fail.
      return [...plan.querySelectorAll('td, .chip')]
        .filter((el) => el.checkVisibility())
        .map((el) => ({
          word: el.textContent.trim().slice(0, 28),
          spill: Math.round(Math.max(0, el.scrollWidth - el.clientWidth)),
        }));
    });
    if (!measured) {
      fail(`${path} at ${width}px: the plan is missing`);
      continue;
    }
    for (const { word, spill } of measured) {
      planCells += 1;
      if (spill > 0) fail(`${path} at ${width}px: "${word}" is clipped by ${spill}px`);
    }
  }
  await tab.close();
}
// The count is the guard on the guard: a selector that stops matching would
// measure nothing and pass.
check(planCells >= 400, `measured ${planCells} plan cell(s) across the landings, expected at least 400`);

// The dark palette is a stamped choice, so the sweep above never sees it. Each
// page once more in dark, at a desktop width, where every block is on screen.
for (const path of PAGES) {
  const tab = await context.newPage();
  await tab.addInitScript(() => localStorage.setItem('routarr.theme', 'dark'));
  await tab.setViewportSize({ width: 1440, height: 900 });
  await tab.goto(`${BASE}${path}`, { waitUntil: 'networkidle' });
  const dark = await tab.evaluate(() => document.documentElement.dataset.theme === 'dark');
  check(dark, `${path} did not open in the dark theme, so the dark sweep read the light one`);
  const { violations } = await new AxeBuilder({ page: tab })
    .withTags(['wcag2a', 'wcag2aa', 'wcag21a', 'wcag21aa'])
    .analyze();
  for (const violation of violations) {
    const where = violation.nodes.map((node) => node.target.join(' ')).slice(0, 3).join(', ');
    fail(`${path} in dark: ${violation.id} (${violation.impact}), ${where}`);
  }
  await tab.close();
}

// ------------------------------------------------------------ theme
// Before anyone touches it, the switch says the theme on screen: a screen
// reader reads its state, not its colour. Nothing stamped is the light default.
const announced = await page.evaluate(() =>
  [...document.querySelectorAll('[data-theme-set]')]
    .filter((button) => button.getAttribute('aria-pressed') === 'true')
    .map((button) => button.getAttribute('data-theme-set')),
);
const onScreen = await page.evaluate(() =>
  document.documentElement.dataset.theme === 'dark' ? 'dark' : 'light',
);
check(
  announced.length > 0 && announced.every((theme) => theme === onScreen),
  `the theme switch announces ${announced.join(', ') || 'nothing'} on a ${onScreen} page`,
);

// Both states are named, so the test asks for the one the page is not in:
// clicking the lit cell is a no-op by design and would report a dead control.
const before = await page.evaluate(() => getComputedStyle(document.body).backgroundColor);
// Nothing is stamped on the default, which is light, so the question is
// whether dark was chosen, not whether light was.
const other = await page.evaluate(() =>
  document.documentElement.dataset.theme === 'dark' ? 'light' : 'dark',
);
// The switch sits inside the Index panel, off the bar, so the panel is opened
// first.
await page.click('.index > summary');
await page.click(`[data-theme-set="${other}"]`);
await page.waitForTimeout(120);
const after = await page.evaluate(() => getComputedStyle(document.body).backgroundColor);
check(before !== after, 'the theme switch changed nothing');
check(
  await page.evaluate((want) => document.querySelector(`[data-theme-set="${want}"]`).getAttribute('aria-pressed') === 'true', other),
  'the theme switch does not say which state it is in',
);

// The browser's bar takes the ground of the theme on screen, on the click and
// again on the next load.
const barMatches = () =>
  page.evaluate(
    () => document.querySelector('meta[name="theme-color"]').content === getComputedStyle(document.body).backgroundColor
      .match(/\d+/g).slice(0, 3).reduce((hex, channel) => hex + Number(channel).toString(16).padStart(2, '0'), '#'),
  );
check(await barMatches(), 'theme-color does not follow the theme picked');

const chosen = await page.evaluate(() => document.documentElement.dataset.theme);
await page.reload({ waitUntil: 'networkidle' });
const persisted = await page.evaluate(() => document.documentElement.dataset.theme);
check(persisted === chosen, `the theme choice did not survive a reload (${chosen} became ${persisted})`);
check(await barMatches(), 'theme-color does not follow the theme a reload restored');

// ------------------------------------------------------------ copy
// A second click inside the delay must not take "Copied" for the label to put
// back, or the button says it for good.
{
  const tab = await context.newPage();
  await tab.goto(`${BASE}/`, { waitUntil: 'networkidle' });
  const copy = tab.locator('.copy-btn').first();
  const label = await copy.innerText();
  await copy.click();
  await copy.click();
  await tab.waitForTimeout(1900);
  const now = await copy.innerText();
  check(now === label, `the Copy button reads "${now}" after two clicks, not "${label}"`);
  await tab.close();
}

// ------------------------------------------------------------ focus rings
// A scrolling region takes the focus so the keyboard can scroll it. Its ring,
// drawn outside it, is cut off by a card that clips its overflow, and the
// reader cannot see where the focus went. Walked with Tab, as a reader does,
// on each page that has such a region: the plan on the landing, the call and
// its answer on the API page.
for (const [path, width] of ['/', '/api/'].flatMap((path) => [[path, 1440], [path, 375]])) {
  const tab = await context.newPage();
  await tab.setViewportSize({ width, height: 900 });
  await tab.goto(`${BASE}${path}`, { waitUntil: 'networkidle' });
  const cut = new Set();
  let regions = 0;
  for (let step = 0; step < 120; step++) {
    await tab.keyboard.press('Tab');
    const found = await tab.evaluate(() => {
      const element = document.activeElement;
      if (!element || element.getAttribute('tabindex') !== '0') return null;
      const style = getComputedStyle(element);
      const reach = parseFloat(style.outlineOffset) + parseFloat(style.outlineWidth);
      const box = element.getBoundingClientRect();
      for (let parent = element.parentElement; parent; parent = parent.parentElement) {
        if (getComputedStyle(parent).overflow === 'visible') continue;
        const clip = parent.getBoundingClientRect();
        if (box.left - reach < clip.left || box.right + reach > clip.right
          || box.top - reach < clip.top || box.bottom + reach > clip.bottom) {
          return { cut: element.className || element.tagName.toLowerCase() };
        }
      }
      return { cut: null };
    });
    if (!found) continue;
    regions++;
    if (found.cut) cut.add(found.cut);
  }
  check(regions > 0, `${path} at ${width}px: the Tab walk reached no scrolling region, so the ring check read nothing`);
  check(cut.size === 0, `${path} at ${width}px: a card cuts off the focus ring of: ${[...cut].join(', ')}`);
  await tab.close();
}

// ------------------------------------------------------------ anchors
// A section the Index panel names lands below the sticky header, its heading
// in view, not under the bar that covers the first lines.
{
  const tab = await context.newPage();
  await tab.emulateMedia({ reducedMotion: 'reduce' });
  await tab.setViewportSize({ width: 390, height: 800 });
  let anchors = 0;
  for (const path of [...LANDINGS.slice(0, 1), ...DETAILS.slice(0, 1)]) {
    await tab.goto(`${BASE}${path}`, { waitUntil: 'networkidle' });
    const targets = await tab.evaluate(() =>
      [...document.querySelectorAll('.index-nav a[href^="#"]')].map((link) => link.getAttribute('href').slice(1)));
    for (const id of targets) {
      await tab.goto(`${BASE}${path}#${id}`, { waitUntil: 'networkidle' });
      const { top, bar } = await tab.evaluate((target) => ({
        top: document.getElementById(target).getBoundingClientRect().top,
        bar: document.querySelector('.site-header').getBoundingClientRect().bottom,
      }), id);
      anchors++;
      check(top >= bar - 1, `${path}#${id} lands ${Math.round(bar - top)}px under the header`);
    }
  }
  check(anchors >= 5, `followed ${anchors} anchor(s), so the landing check read nothing`);
  await tab.close();
}

// ------------------------------------------------------------ index panel
// On a phone the destinations stack in one column: two narrow columns fold
// every title and every description.
{
  const tab = await context.newPage();
  await tab.setViewportSize({ width: 375, height: 800 });
  await tab.goto(`${BASE}/`, { waitUntil: 'networkidle' });
  const columns = await tab.evaluate(() => {
    const index = document.querySelector('.index');
    if (index) index.open = true;
    const nav = document.querySelector('.index-panel .index-nav');
    return nav ? getComputedStyle(nav).gridTemplateColumns.split(' ').length : 0;
  });
  check(columns === 1, `the Index panel keeps ${columns} columns on a phone`);
  await tab.close();
}

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

// --------------------------------------------------- the language offer
// `site.js` reads `navigator.languages` and offers the reader's own language in
// a banner. Nothing else exercises it: the pages render identically whether it
// works or not, so a broken offer goes unseen.
//
// Each case takes a context of its own. The offer writes the choice to
// `localStorage` and returns early when it finds one, so a reused context makes
// every case after the first pass for the wrong reason.
//
// The expected sentence is read off the switcher's own `data-offer` rather than
// written here: what is being asked is whether the mechanism picks the language
// the browser asked for, and a second copy of four sentences would only drift.
async function offered(path, locale) {
  const ctx = await browser.newContext({ locale, viewport: { width: 1280, height: 800 } });
  const tab = await ctx.newPage();
  await tab.goto(`${BASE}${path}`, { waitUntil: 'networkidle' });
  const hint = tab.locator('#lang-hint');
  const shown = await hint.isVisible();
  const text = shown ? (await hint.innerText()).split('\n')[0].trim() : null;
  return { ctx, tab, hint, shown, text };
}

for (const [path, locale] of [['/', 'fr-FR'], ['/', 'de'], ['/how/', 'es-MX']]) {
  const { ctx, tab, shown, text } = await offered(path, locale);
  const want = await tab.evaluate(
    (code) => document.querySelector(`.lang-nav a[hreflang="${code}"]`)?.getAttribute('data-offer'),
    locale.split('-')[0],
  );
  check(shown, `${path} offers nothing to a ${locale} browser`);
  check(text === want, `${path} offered "${text}" to a ${locale} browser, expected "${want}"`);
  // Read aloud with the rules of the language it is written in (WCAG 3.1.2).
  const spoken = await tab.evaluate(() => document.querySelector('#lang-hint a')?.getAttribute('lang'));
  check(spoken === locale.split('-')[0], `${path} offers its ${locale} link in lang "${spoken}"`);
  await ctx.close();
}

// Silent where it has nothing to say: on a page already in that language, and
// for a language the site does not speak. A banner in either case is worse than
// none, since it sends the reader somewhere they already are or nowhere.
for (const [path, locale, why] of [
  ['/fr/', 'fr', 'the page is already French'],
  ['/', 'ja', 'the site does not speak Japanese'],
]) {
  const { ctx, shown, text } = await offered(path, locale);
  check(!shown, `${path} offered "${text}" to a ${locale} browser though ${why}`);
  await ctx.close();
}

// Dismissing is a decision, and it outranks the browser from then on. Without
// this the banner argues on every page of the visit.
{
  const { ctx, tab, hint, shown } = await offered('/', 'fr-FR');
  // The button clicked is the one the banner rendered, never one looked up by a
  // name this file guessed: asked for "Fermer" while the banner had offered
  // German, the locator spends its whole timeout and throws, which kills the
  // run before the failures already recorded above are ever printed. Its name
  // is worth asserting, and that is a check rather than a way to find it.
  const button = hint.getByRole('button');
  if (!shown || (await button.count()) !== 1) {
    fail(`the language offer rendered ${shown ? 'no single button' : 'nothing'}, so dismissing it could not be checked`);
  } else {
    const want = await tab.evaluate(
      () => document.querySelector('.lang-nav a[hreflang="fr"]')?.getAttribute('data-dismiss'),
    );
    const named = await button.getAttribute('aria-label');
    check(named === want, `the dismissal is named "${named}", expected "${want}"`);
    await button.click({ timeout: 5000 });
    check(!(await hint.isVisible()), 'dismissing the language offer left it on screen');
    await tab.reload({ waitUntil: 'networkidle' });
    check(!(await hint.isVisible()), 'the language offer came back after a reload it had been dismissed on');
  }
  await ctx.close();
}

// ------------------------------------------------------------ 404
// A miss under a language's prefix is answered in that language, and one
// outside every prefix in English, as Cloudflare serves the nearest 404.html.
for (const { code, path } of LANGUAGES) {
  const missing = await page.goto(`${BASE}${path}does-not-exist`);
  check(missing.status() === 404, `${path}does-not-exist answered ${missing.status()}, expected 404`);
  const lang = await page.evaluate(() => document.documentElement.lang);
  check(lang === code, `a miss under ${path} is answered in "${lang}", not "${code}"`);
  check((await page.locator('h1').count()) === 1, `the ${code} not-found page does not have exactly one heading`);
}

await browser.close();
stop();

if (failures.length) {
  console.error(`\n${failures.length} problem(s):\n`);
  for (const problem of failures) console.error(`  - ${problem}`);
  process.exit(1);
}
console.log('site verified in a browser');
