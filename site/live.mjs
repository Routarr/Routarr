#!/usr/bin/env node
/**
 * Compares production with this checkout, the release `site.yml` deploys.
 *
 * The Cloudflare zone adds to every response at the edge, where no build and
 * no preview can see it. What it adds on purpose is listed below, and anything
 * else fails: a header `_headers` sets and production serves otherwise, a
 * header nobody declared, a script from another origin, a CSP violation.
 *
 *   node site/live.mjs [origin]
 */
import { readFileSync } from 'node:fs';
import { join } from 'node:path';
import { fileURLToPath } from 'node:url';

import { headersFor, parseHeaders } from './headers.mjs';
import { chromium } from './playwright.mjs';

const ROOT = fileURLToPath(new URL('.', import.meta.url));
const ORIGIN =
  process.argv[2] ?? readFileSync(join(ROOT, 'astro.config.mjs'), 'utf-8').match(/^\s*site: '([^']+)'/m)[1];
const VERSION = readFileSync(join(ROOT, '../backend/Cargo.toml'), 'utf-8').match(/^version = "([^"]+)"/m)[1];
const RULES = parseHeaders(readFileSync(join(ROOT, 'public/_headers'), 'utf-8'));
const { LANGUAGES } = await import('./src/i18n/languages.ts');

/** What the zone adds and the project keeps: Network Error Logging and Speed Brain. */
const ZONE = new Set(['nel', 'report-to', 'speculation-rules']);
/** What any response through Cloudflare carries. */
const TRANSPORT = new Set([
  'accept-ranges', 'age', 'alt-svc', 'cf-cache-status', 'cf-ray', 'content-encoding', 'content-length',
  'content-type', 'date', 'etag', 'last-modified', 'priority', 'server', 'server-timing', 'vary',
]);
/** The Web Analytics beacon the zone injects, the one script from another origin. */
const BEACON = 'https://static.cloudflareinsights.com/beacon.min.js/';

const failures = [];
const fail = (message) => failures.push(message);

const browser = await chromium.launch();
// The browser's own version under an ordinary agent: the zone injects the
// beacon into what it takes for a browser, and may challenge a headless one.
const agent = `Mozilla/5.0 (X11; Linux x86_64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/${browser.version()} Safari/537.36`;

// A deploy reaches every edge within seconds, and a page cached before it is
// revalidated: production names the release once the switch has happened.
const deadline = Date.now() + 180_000;
for (;;) {
  const page = await fetch(`${ORIGIN}/`, { headers: { 'user-agent': agent } }).then((r) => r.text(), () => '');
  if (page.includes(`v${VERSION}`)) break;
  if (Date.now() > deadline) {
    console.error(`${ORIGIN}/ still names no v${VERSION} after three minutes`);
    process.exit(1);
  }
  await new Promise((resolve) => setTimeout(resolve, 10_000));
}

const context = await browser.newContext({ userAgent: agent });
await context.addInitScript(() => {
  window.__csp = [];
  document.addEventListener('securitypolicyviolation', (e) => {
    window.__csp.push(`${e.violatedDirective} blocked ${e.blockedURI}`);
  });
});
// The beacon's report is answered here, so this visit is not counted as one.
await context.route(`${ORIGIN}/cdn-cgi/rum*`, (route) => route.fulfill({ status: 204 }));

const PAGES = LANGUAGES.flatMap(({ path }) => [path, `${path}how/`, `${path}api/`]);
for (const path of PAGES) {
  const tab = await context.newPage();
  const problems = [];
  const foreign = [];
  let beacon = false;
  tab.on('console', (message) => {
    if (message.type() === 'error') problems.push(`console: ${message.text()}`);
  });
  tab.on('pageerror', (error) => problems.push(`uncaught: ${error.message}`));
  tab.on('requestfailed', (request) => problems.push(`failed request: ${request.url()}`));
  tab.on('response', (response) => {
    const url = response.url();
    if (url.startsWith(BEACON)) beacon = response.ok();
    else if (!url.startsWith(`${ORIGIN}/`)) foreign.push(url);
  });

  const response = await tab.goto(`${ORIGIN}${path}`, { waitUntil: 'networkidle' });
  if (response?.status() !== 200) fail(`${path} answered ${response?.status()}`);
  const served = (await response?.allHeaders()) ?? {};
  const declared = headersFor(RULES, path);
  for (const [name, value] of Object.entries(declared)) {
    const actual = served[name.toLowerCase()];
    if (actual !== value) fail(`${path}: ${name} is served as "${actual ?? '(absent)'}", _headers says "${value}"`);
  }
  const known = new Set(Object.keys(declared).map((name) => name.toLowerCase()));
  for (const name of Object.keys(served)) {
    if (!known.has(name) && !ZONE.has(name) && !TRANSPORT.has(name)) fail(`${path}: production adds ${name}`);
  }

  if (!beacon) fail(`${path}: the Web Analytics beacon did not load`);
  for (const url of foreign) fail(`${path}: loads ${url}`);
  for (const violation of await tab.evaluate(() => window.__csp)) fail(`${path}: content-security-policy ${violation}`);
  for (const problem of problems) fail(`${path}: ${problem}`);
  await tab.close();
}

// A miss under a language's prefix is answered in that language.
for (const { code, path } of LANGUAGES) {
  const tab = await context.newPage();
  const response = await tab.goto(`${ORIGIN}${path}does-not-exist`);
  if (response?.status() !== 404) fail(`${path}does-not-exist answered ${response?.status()}, not 404`);
  const lang = await tab.evaluate(() => document.documentElement.lang);
  if (lang !== code) fail(`a miss under ${path} is answered in "${lang}", not "${code}"`);
  await tab.close();
}

await browser.close();

if (failures.length) {
  console.error(`\n${failures.length} problem(s) in production:\n`);
  for (const problem of failures) console.error(`  - ${problem}`);
  process.exit(1);
}
console.log(`${ORIGIN} serves v${VERSION} as this checkout declares it`);
