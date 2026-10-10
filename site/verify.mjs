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
import { readFileSync } from 'node:fs';
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
// Every page at WCAG 2.2 AA, in both themes, at a desktop and a phone width:
// what the application's own sweep holds itself to, held here as well. Each
// page is read as it opens, then with every disclosure of its content open,
// since a folded panel hides its contents from axe as much as from the
// reader. The Index panel is then read open on its own, over the page it
// overlays.
/* The content pages: every language of the landing and of the detail page,
   from the one list the pages are built from, because a probe that quietly
   stops covering half the site is the kind that keeps passing. */
const { LANGUAGES } = await import('./src/i18n/languages.ts');
const LANDINGS = LANGUAGES.map(({ path }) => path);
const DETAILS = LANDINGS.map((path) => `${path}how/`);
const APIS = LANDINGS.map((path) => `${path}api/`);
const PAGES = [...LANDINGS, ...DETAILS, ...APIS];
const AXE_TAGS = ['wcag2a', 'wcag2aa', 'wcag21a', 'wcag21aa', 'wcag22aa'];

/** axe's violations, and the contrasts it could not measure, on a page as it stands. */
async function audit(tab, where, within) {
  const axe = new AxeBuilder({ page: tab }).withTags(AXE_TAGS);
  const { violations, incomplete } = await (within ? axe.include(within) : axe).analyze();
  for (const violation of violations) {
    const nodes = violation.nodes.map((node) => node.target.join(' ')).slice(0, 3).join(', ');
    fail(`${where}: ${violation.id} (${violation.impact}), ${nodes}`);
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
    const nodes = unmeasured.map((node) => node.target.join(' ')).slice(0, 3).join(', ');
    fail(`${where}: a contrast axe could not measure, ${nodes}`);
  }
}

const NOT_FOUND = LANDINGS.map((path) => `${path}404.html`);
let audited = 0;
// Dark is what every page opens in, and light is the visitor's stamped choice.
for (const theme of ['dark', 'light']) {
  for (const path of [...PAGES, ...NOT_FOUND]) {
    for (const width of [1440, 375]) {
      const tab = await context.newPage();
      if (theme === 'light') await tab.addInitScript(() => localStorage.setItem('routarr.theme', 'light'));
      await tab.setViewportSize({ width, height: 900 });
      const before = problems.length;
      await tab.goto(`${BASE}${path}`, { waitUntil: 'networkidle' });
      const where = `${path} at ${width}px in ${theme}`;
      const shown = await tab.evaluate(() => document.documentElement.dataset.theme);
      check(shown === theme, `${where} opened in ${shown}, so the sweep read the other theme`);
      const blocked = await tab.evaluate(() => window.__csp);
      for (const violation of blocked) fail(`${where}: content-security-policy ${violation}`);
      for (const problem of problems.slice(before)) fail(`${where}: ${problem}`);
      await audit(tab, where);
      const disclosures = await tab.evaluate(() => {
        const all = document.querySelectorAll('main details');
        for (const disclosure of all) disclosure.open = true;
        return all.length;
      });
      if (disclosures) await audit(tab, `${where}, every disclosure open`);
      const index = await tab.evaluate(() => {
        const panel = document.querySelector('.index');
        if (!panel) return false;
        panel.open = true;
        // axe takes text lying under the panel's opaque ground for a contrast
        // it cannot measure, so the page beneath is hidden for this read.
        document.querySelector('main').style.visibility = 'hidden';
        return true;
      });
      if (index) await audit(tab, `${where}, the Index open`, '.index');
      audited += 1;
      await tab.close();
    }
  }
}
check(audited === (PAGES.length + NOT_FOUND.length) * 4, `audited ${audited} page state(s), so a sweep was skipped`);

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
// above passes. Down to 320px (the width WCAG reflow names), and across the
// band where the layouts change, every box is measured against its own
// content, with the copy button showing its longest label. Scroll regions
// scroll by design, an ellipsis cuts on purpose, and a glyph may reach past a
// tight line box by a few pixels, more in a large heading.
/** The boxes of a page whose content reaches past them, and how many were measured. */
function clippedBoxes(tab) {
  return tab.evaluate(() => {
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
      const reach = Math.max(4, parseFloat(style.fontSize) * 0.12);
      if (across > 1 || down > (hides ? 1 : reach)) {
        const name = `${el.tagName.toLowerCase()}${[...el.classList].map((c) => `.${c}`).join('')}`;
        clipped.push(`${name} "${el.textContent.trim().slice(0, 30)}" by ${across}x${down}px`);
      }
    }
    return { count, clipped };
  });
}

/** Every page at each width, `prepare` run once it has loaded. */
async function sweepClipping(on, widths, label, prepare = async () => {}) {
  let measured = 0;
  for (const path of PAGES) {
    const tab = await on.newPage();
    await tab.goto(`${BASE}${path}`, { waitUntil: 'networkidle' });
    await tab.evaluate(() => document.querySelectorAll('.copy-btn').forEach((button) => {
      button.textContent = button.dataset.failed;
    }));
    await prepare(tab);
    for (const width of widths) {
      await tab.setViewportSize({ width, height: 900 });
      await tab.waitForTimeout(80);
      const { count, clipped } = await clippedBoxes(tab);
      measured += count;
      for (const box of clipped) fail(`${path} at ${width}px${label}: ${box} beyond its box`);
    }
    await tab.close();
  }
  return measured;
}

const boxesMeasured = await sweepClipping(context, [320, 360, 390, 740, 860, 900, 950, 999], '');
check(boxesMeasured >= 17000, `measured ${boxesMeasured} box(es) for clipping, expected at least 17000`);

// WCAG 1.4.12: a reader's own stylesheet may space the text out, and nothing
// may be lost when it does. Injected past the CSP, which refuses a style.
{
  const spaced = await browser.newContext({ bypassCSP: true });
  const measured = await sweepClipping(spaced, [320, 1280], ' with WCAG text spacing', (tab) => tab.addStyleTag({
    content: '* { line-height: 1.5 !important; letter-spacing: 0.12em !important; word-spacing: 0.16em !important; }'
      + ' p { margin-bottom: 2em !important; }',
  }));
  check(measured >= 4000, `measured ${measured} box(es) under text spacing, expected at least 4000`);
  await spaced.close();
}

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

// A move reads as two folders and the word between them, in the page's
// language: the arrow that parts them on screen is hidden from a screen reader.
for (const { code, path } of LANGUAGES) {
  const tab = await context.newPage();
  await tab.goto(`${BASE}${path}`, { waitUntil: 'networkidle' });
  const word = JSON.parse(readFileSync(`${ROOT}src/i18n/${code}.json`, 'utf-8'))['hero.to'];
  const spoken = await tab.locator('.plan-dest:has(.arrow)').first().ariaSnapshot();
  check(spoken.includes(` ${word} `), `${path}: the first move reads "${spoken.trim()}", without "${word}" between its folders`);
  await tab.close();
}

// ------------------------------------------------------------ theme
// Before anyone touches it, the switch says the theme on screen: a screen
// reader reads its state, not its colour.
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

// ------------------------------------------------------------ forced colours
// Forced colours drop the wash and the bar that mark the language of the page,
// the theme on screen and the mode in force: each keeps a mark its neighbour
// does not have.
{
  const tab = await context.newPage();
  await tab.emulateMedia({ forcedColors: 'active' });
  await tab.goto(`${BASE}/`, { waitUntil: 'networkidle' });
  const pairs = await tab.evaluate(() => {
    const outline = (selector) => {
      const element = document.querySelector(selector);
      return element ? getComputedStyle(element).outlineStyle : 'missing';
    };
    return [
      ['the language of the page', outline('.lang-nav a[aria-current="page"]'), outline('.lang-nav a:not([aria-current])')],
      ['the theme on screen', outline('[data-theme-set="dark"]'), outline('[data-theme-set="light"]')],
      ['the mode in force', outline('.rule-mode .on'), outline('.rule-mode > span:not(.on)')],
    ];
  });
  for (const [what, lit, other] of pairs) {
    check(lit !== 'missing' && other !== 'missing' && lit !== other, `in forced colours, ${what} looks like the others (${lit} beside ${other})`);
  }
  await tab.close();
}

// Decoration drawn by CSS is silent, and an operation names its scope as one:
// a screen reader hears neither "slash slash" before a section's label nor a
// triangle before an operation.
{
  const tab = await context.newPage();
  await tab.goto(`${BASE}/api/`, { waitUntil: 'networkidle' });
  const eyebrow = await tab.locator('.eyebrow').first().ariaSnapshot();
  const summary = await tab.locator('.ref-op > summary').first().ariaSnapshot();
  check(!eyebrow.includes('//'), `a section's label reads ${eyebrow.trim()}`);
  check(!summary.includes('\u25B8'), `an operation reads ${summary.trim()}`);
  check(/to ?, scope: read/.test(summary), `an operation runs its scope into its sentence: ${summary.trim()}`);
  await tab.close();
}

// A browser that refuses the clipboard on a secure page gets the code selected,
// so the "Press Ctrl+C" the button then says copies it.
{
  const tab = await context.newPage();
  await tab.addInitScript(() => {
    Object.defineProperty(navigator, 'clipboard', {
      configurable: true,
      value: { writeText: () => Promise.reject(new Error('refused')) },
    });
  });
  await tab.goto(`${BASE}/`, { waitUntil: 'networkidle' });
  const copy = tab.locator('.copy-btn').first();
  const label = await copy.innerText();
  await copy.click();
  await tab.waitForFunction((before) => document.querySelector('.copy-btn').textContent !== before, label);
  const { said, failed, selected, code } = await copy.evaluate((button) => ({
    said: button.textContent,
    failed: [button.dataset.failed, button.dataset.failedMac],
    selected: String(getSelection()),
    code: button.closest('.terminal').querySelector('code').innerText,
  }));
  check(failed.includes(said), `a refused copy reads "${said}", not the shortcut to press`);
  check(selected === code, `a refused copy selects ${selected.length} character(s) of the ${code.length} the shortcut should copy`);
  await tab.close();
}

// ------------------------------------------------------------ analytics
// Production carries the Cloudflare Web Analytics beacon, which the zone
// injects at the edge, so no build here has it. The tag the zone writes is
// added to a page, its script and its report answered by stand-ins, and the
// CSP of `_headers` must admit the script and what it sends to the page's
// own `/cdn-cgi/rum`.
{
  const tab = await context.newPage();
  let reported = false;
  await tab.route('https://static.cloudflareinsights.com/**', (route) =>
    route.fulfill({ contentType: 'text/javascript', body: "navigator.sendBeacon('/cdn-cgi/rum', '{}');" }));
  await tab.route('**/cdn-cgi/rum', (route) => {
    reported = true;
    return route.fulfill({ status: 204 });
  });
  await tab.goto(`${BASE}/`, { waitUntil: 'networkidle' });
  const loaded = await tab.evaluate(() => new Promise((resolve) => {
    const beacon = document.createElement('script');
    beacon.type = 'module';
    beacon.crossOrigin = 'anonymous';
    beacon.src = 'https://static.cloudflareinsights.com/beacon.min.js/v4bc70e2c01a94c73b74392e4234840661791215815920';
    beacon.onload = () => resolve(true);
    beacon.onerror = () => resolve(false);
    document.body.append(beacon);
  }));
  await tab.waitForTimeout(300);
  const blocked = await tab.evaluate(() => window.__csp);
  check(loaded, 'the CSP refuses the Web Analytics beacon the zone injects');
  check(reported, 'the Web Analytics beacon could not send its report');
  for (const violation of blocked) fail(`the Web Analytics beacon: content-security-policy ${violation}`);
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

// ------------------------------------------------------------ reference anchors
// An operation or a type is linked by its id, and its entry opens on arrival
// and when a type name on the page is followed. A fragment mangled on its way
// to the page leaves the script running.
{
  const tab = await context.newPage();
  const before = problems.length;
  // `hashchange` is a task of its own, so the entry opens a moment after the
  // click that moved the fragment.
  const openedByHash = () => tab
    .waitForFunction(() => {
      const target = document.getElementById(decodeURIComponent(location.hash.slice(1)));
      return target?.tagName === 'DETAILS' && target.open;
    }, null, { timeout: 2000 })
    .then(() => true, () => false);
  await tab.goto(`${BASE}/api/#place_title`, { waitUntil: 'networkidle' });
  if (!(await openedByHash())) {
    fail('/api/#place_title does not open that operation');
  } else {
    await tab.locator('#place_title a[href^="#schema-"]').first().click();
    check(await openedByHash(), 'following a type name does not open its entry');
  }
  await tab.goto(`${BASE}/how/#%E0%A4%A`, { waitUntil: 'networkidle' });
  for (const problem of problems.slice(before)) fail(`reference anchors: ${problem}`);
  await tab.close();
}

// ------------------------------------------------------------ index panel
// The open panel keeps the page's margins: its edge meets the bar's last
// control, and a phone leaves room on its other side too.
for (const width of [375, 1440]) {
  const tab = await context.newPage();
  await tab.setViewportSize({ width, height: 800 });
  await tab.goto(`${BASE}/`, { waitUntil: 'networkidle' });
  const edges = await tab.evaluate(() => {
    document.querySelector('.index').open = true;
    const panel = document.querySelector('.index-panel').getBoundingClientRect();
    const last = document.querySelector('.site-header .wrap > :last-child').getBoundingClientRect();
    return { left: panel.left, right: panel.right, control: last.right };
  });
  check(Math.abs(edges.right - edges.control) <= 1, `at ${width}px the Index panel ends at ${edges.right}, the bar's last control at ${edges.control}`);
  check(edges.left >= 16, `at ${width}px the Index panel starts ${edges.left}px from the edge`);
  await tab.close();
}

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

// Each language in the switcher is read in that language, and named in it.
{
  const switcher = await page.evaluate(() =>
    [...document.querySelectorAll('.lang-nav a')].map((link) => ({
      code: link.getAttribute('hreflang'),
      lang: link.getAttribute('lang'),
      name: link.querySelector('.sr-only')?.textContent.trim() ?? '',
    })));
  check(switcher.length === LANGUAGES.length, `the switcher offers ${switcher.length} language(s)`);
  for (const { code, lang, name } of switcher) {
    check(lang === code, `the switcher's ${code} is read in lang "${lang}"`);
    check(name === LANGUAGES.find((language) => language.code === code)?.name, `the switcher's ${code} is named "${name}"`);
  }
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
    const spoken = await button.getAttribute('lang');
    check(spoken === 'fr', `the dismissal "${named}" is read in lang "${spoken}", not "fr"`);
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
