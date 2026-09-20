#!/usr/bin/env node
/**
 * Static checks on the showcase site.
 *
 * These are the mistakes that survive a visual review: a canonical pointing at
 * the wrong host after a domain change, a CSP hash left behind by an edited
 * script, an <img> with no dimensions quietly shifting the layout, a link to a
 * file that is not deployed.
 *
 *   node site/check.mjs
 */
import { readFileSync, existsSync, readdirSync } from 'node:fs';
import { createHash } from 'node:crypto';
import { join } from 'node:path';
import { fileURLToPath } from 'node:url';

const ROOT = fileURLToPath(new URL('.', import.meta.url));

/**
 * Everything here is checked against `dist/`, which is what Cloudflare serves.
 * Checking the sources would test the intention; this tests the artefact — the
 * distinction that matters for a CSP hash, a fingerprinted stylesheet, or an
 * image that exists in `public/` and never reached the build.
 */
const DIST = join(ROOT, 'dist');
const read = (file) => readFileSync(join(DIST, file), 'utf-8');

if (!existsSync(DIST)) {
  console.error('site/dist is missing — run `npm run build` in site/ first.');
  process.exit(1);
}

// The one list of languages, read from the source the pages are built from:
// a copy here drifted the first time a language was added elsewhere.
const { LANGUAGES } = await import('./src/i18n/languages.ts');


/** Read a WebP's intrinsic size without pulling in an image library. */
function webpSize(file) {
  const buffer = readFileSync(file);
  if (buffer.toString('ascii', 0, 4) !== 'RIFF' || buffer.toString('ascii', 8, 12) !== 'WEBP') return null;
  const chunk = buffer.toString('ascii', 12, 16);
  if (chunk === 'VP8X') {
    return { width: buffer.readUIntLE(24, 3) + 1, height: buffer.readUIntLE(27, 3) + 1 };
  }
  if (chunk === 'VP8 ') {
    return { width: buffer.readUInt16LE(26) & 0x3fff, height: buffer.readUInt16LE(28) & 0x3fff };
  }
  if (chunk === 'VP8L') {
    const bits = buffer.readUInt32LE(21);
    return { width: (bits & 0x3fff) + 1, height: ((bits >> 14) & 0x3fff) + 1 };
  }
  return null;
}

/**
 * Same, for an AVIF: from its `ispe` property box. The two formats are encoded
 * from one PNG, so a stale AVIF beside a fresh WebP is what a re-capture that
 * failed halfway leaves behind — and the AVIF is the file most browsers take.
 */
function avifSize(file) {
  const buffer = readFileSync(file);
  const at = buffer.indexOf('ispe');
  if (at < 0 || at + 16 > buffer.length) return null;
  // Box name, then a version/flags word, then width and height.
  return { width: buffer.readUInt32BE(at + 8), height: buffer.readUInt32BE(at + 12) };
}

/** Same, for a PNG: width and height sit in the IHDR, right after the signature. */
function pngSize(file) {
  if (!existsSync(file)) return null;
  const buffer = readFileSync(file);
  if (buffer.toString('hex', 0, 8) !== '89504e470d0a1a0a') return null;
  return { width: buffer.readUInt32BE(16), height: buffer.readUInt32BE(20) };
}

const failures = [];
const fail = (message) => failures.push(message);

const index = read('index.html');
const notFound = read('404.html');
const headers = read('_headers');
const pages = { 'index.html': index, '404.html': notFound };


// -------------------------------------------------------------- one origin
// The site states its own origin in the canonical, the sitemap and robots.txt.
// A partial rename — one updated, another forgotten — is the failure mode.
const origins = new Set();
for (const source of [index, notFound, headers, read('robots.txt'), read('sitemap-index.xml')]) {
  for (const [, origin] of source.matchAll(/https?:\/\/([a-z0-9.-]+)/gi)) origins.add(origin);
}
// `localhost` appears in the install instructions as prose, not as a host the
// site talks to; the rest are well-known vocabulary and outbound links.
const EXTERNAL = /(github|schema\.org|gnu\.org|ogp\.me|sitemaps\.org|w3\.org|localhost|127\.0\.0\.1)/;
const own = [...origins].filter((o) => !EXTERNAL.test(o));
if (own.length > 1) fail(`several site origins in use, pick one: ${own.join(', ')}`);
const ORIGIN = own[0];

// ------------------------------------------------------------ translations
// Astro renders the four pages from one set of components, so a translation
// cannot be *stale* — there is no second copy of the page to fall behind.
// What is possible is a catalogue that has drifted from the keys the
// components ask for, so that is what this checks: same keys as English, none
// empty, and a built page at the path the switcher and the sitemap promise.
const catalogue = (code) =>
  JSON.parse(readFileSync(join(ROOT, `src/i18n/${code}.json`), 'utf-8'));

const english = catalogue('en');
for (const { code, path } of LANGUAGES) {
  const file = code === 'en' ? 'index.html' : `${code}/index.html`;

  if (!existsSync(join(ROOT, `src/i18n/${code}.json`))) {
    fail(`${code} is in LANGUAGES but has no src/i18n/${code}.json`);
    continue;
  }
  const dictionary = catalogue(code);

  const missing = Object.keys(english).filter((key) => !(key in dictionary));
  const unknown = Object.keys(dictionary).filter((key) => !(key in english));
  if (missing.length) {
    fail(`src/i18n/${code}.json is missing ${missing.length} key(s): ${missing.slice(0, 3).join(', ')}…`);
  }
  if (unknown.length) {
    fail(`src/i18n/${code}.json has ${unknown.length} key(s) English does not: ${unknown.slice(0, 3).join(', ')}…`);
  }
  for (const [key, value] of Object.entries(dictionary)) {
    if (!String(value).trim()) fail(`src/i18n/${code}.json: ${key} is empty`);
  }

  if (!existsSync(join(DIST, file))) {
    fail(`${file} was not built — run \`npm run build\` in site/`);
    continue;
  }
  pages[file] = read(file);

  if (!index.includes(`href="${path}" hreflang="${code}"`)) {
    fail(`the built page does not offer ${path} in its language switcher`);
  }
  if (!index.includes(`<link rel="alternate" hreflang="${code}" href="https://${ORIGIN}${path}">`)) {
    fail(`the built page has no hreflang alternate for ${code}`);
  }
}


// ------------------------------------------------ chrome outside the catalogue
// An `aria-label`, a `<b>` in a list, a `//` eyebrow: text written straight into
// a component is invisible to the key parity above and ships in English on the
// three other pages. Every label a translated page announces has to differ
// from the English one, and the phrases that once escaped stay named.
const labelsOf = (html) =>
  new Set([...html.matchAll(/aria-label="([^"]*)"/g)].map((m) => m[1]));
const englishLabels = labelsOf(index);
if (englishLabels.size < 4) {
  fail(`only ${englishLabels.size} aria-label(s) on the English page — the guard read nothing`);
}
const ESCAPED = ['one compose up', 'Global dry-run', 'Batch cap', '// before', 'Press Ctrl+C', 'Not affiliated'];
for (const { code } of LANGUAGES) {
  const page = pages[`${code}/index.html`];
  if (code === 'en' || !page) continue;
  for (const label of labelsOf(page)) {
    if (englishLabels.has(label)) fail(`${code}/index.html announces "${label}" in English`);
  }
  for (const phrase of ESCAPED) {
    if (page.includes(phrase)) fail(`${code}/index.html still says "${phrase}" in English`);
  }
}

// ---------------------------------------------------------- words in CSS
// A word a stylesheet prints through `content` never reaches a catalogue, so it
// reads in English on all four pages — and the check above cannot see it,
// since the key it would need does not exist. Symbols are fine there; letters
// are not. CSS escapes are removed first: `\2212` is a minus sign, not text.
const stylesheets = readdirSync(join(DIST, '_astro')).filter((file) => file.endsWith('.css'));
let printed = 0;
for (const sheet of stylesheets) {
  for (const [, , value] of read(`_astro/${sheet}`).matchAll(/content:\s*(["'])((?:\\.|(?!\1).)*)\1/g)) {
    printed += 1;
    if (/\p{L}/u.test(value.replace(/\\[0-9a-fA-F]{1,6}\s?/g, ''))) {
      fail(`_astro/${sheet} prints "${value}" through CSS content, which no translation reaches`);
    }
  }
}
// The count is the guard on the guard: a pattern that stops matching minified
// output would pass having read nothing.
if (!stylesheets.length || printed < 3) {
  fail(`read ${printed} CSS content string(s) in ${stylesheets.length} stylesheet(s) — the check is reading nothing`);
}

// ----------------------------------------------------------- version
// The site states a version in two places and the application owns it in a
// third. Unconnected, the badge sits a release behind and the page still passes
// every other check here.
const cargo = readFileSync(join(ROOT, '../backend/Cargo.toml'), 'utf-8');
const version = cargo.match(/^version = "([^"]+)"/m)?.[1];
if (!version) fail('could not read the version from Cargo.toml');
else {
  if (!index.includes(`"softwareVersion": "${version}"`)) {
    fail(`index.html: softwareVersion is not ${version}, the version in Cargo.toml`);
  }
  if (!index.includes(`<li>v${version}</li>`)) {
    fail(`index.html: the version badge is not v${version}, the version in Cargo.toml`);
  }
}

// ------------------------------------------------------- language offer
// The banner is filled in by site.js from the switcher links, so the two have
// to agree: an id renamed on one side and not the other leaves a reader who
// asked for French looking at English, and nothing anywhere would say so.
const siteScript = read('assets/site.js');
for (const { code } of LANGUAGES) {
  // Matched on the tag rather than on adjacent attributes: attribute order is
  // not meaningful in HTML, and a check that depends on it fails for a reason
  // that has nothing to do with what it is guarding.
  const tag = index.match(new RegExp(`<a [^>]*hreflang="${code}"[^>]*>`))?.[0] ?? '';
  if (!tag) fail(`index.html: no switcher link for ${code}`);
  else {
    if (!tag.includes('data-offer="')) {
      fail(`index.html: the ${code} switcher link carries no data-offer for the banner to read`);
    }
    if (!tag.includes('data-dismiss="')) {
      fail(`index.html: the ${code} switcher link carries no data-dismiss for the banner's close button`);
    }
  }
}
if (!index.includes('id="lang-hint"')) fail('index.html: #lang-hint is missing — site.js builds the banner into it');
if (!siteScript.includes('lang-hint')) fail('assets/site.js never mentions #lang-hint');
if (!/<div class="lang-hint" id="lang-hint"[^>]*\bhidden[^>]*>/.test(index)) {
  fail('index.html: the language banner must start hidden, or a reader without JavaScript sees an empty strip');
}

// -------------------------------------------------------------- csp hash
const inline = [...index.matchAll(/<script>([\s\S]*?)<\/script>/g)].map((m) => m[1]);
if (inline.length !== 1) fail(`expected exactly one inline <script> in index.html, found ${inline.length}`);
for (const script of inline) {
  const digest = `sha256-${createHash('sha256').update(script).digest('base64')}`;
  if (!headers.includes(digest)) {
    fail(`the inline script's hash is not in _headers — the page would render unstyled and dead.\n    expected: ${digest}`);
  }
}
if (![...notFound.matchAll(/<script>([\s\S]*?)<\/script>/g)].every((m) => headers.includes(`sha256-${createHash('sha256').update(m[1]).digest('base64')}`))) {
  fail('404.html carries an inline script whose hash is not in _headers');
}

// -------------------------------------------------------------- head tags
for (const [name, source] of Object.entries(pages)) {
  const title = source.match(/<title>([^<]*)<\/title>/)?.[1] ?? '';
  if (title.length < 15 || title.length > 65) fail(`${name}: title is ${title.length} chars, aim for 15-65`);

  const description = source.match(/<meta name="description" content="([^"]*)"/)?.[1] ?? '';
  if (description.length < 70 || description.length > 165) {
    fail(`${name}: meta description is ${description.length} chars, aim for 70-165`);
  }

  const h1 = [...source.matchAll(/<h1[\s>]/g)].length;
  if (h1 !== 1) fail(`${name}: ${h1} <h1> elements, there must be exactly one`);

  if (!/<html lang="[a-z-]+"/.test(source)) fail(`${name}: <html> has no lang attribute`);
  // `style-src 'self'` blocks every inline style, silently: the page renders
  // without it and only the browser console says so.
  const inlineStyles = [...source.matchAll(/<[a-z][^>]*\sstyle="/g)].length + [...source.matchAll(/<style[\s>]/g)].length;
  if (inlineStyles) fail(`${name}: ${inlineStyles} inline style(s) the CSP will block`);
  if (!source.includes('name="viewport"')) fail(`${name}: no viewport meta`);
}

if (!index.includes(`<link rel="canonical" href="https://${ORIGIN}/">`)) fail('index.html: canonical missing or not the site origin');
if (!index.includes('application/ld+json')) fail('index.html: no structured data');
try {
  JSON.parse(index.match(/<script type="application\/ld\+json">([\s\S]*?)<\/script>/)[1]);
} catch (error) {
  fail(`index.html: the JSON-LD block is not valid JSON (${error.message})`);
}

// -------------------------------------------------------------- images
// Every loop below counts what it examined and refuses to have examined
// nothing: a selector that stops matching, or a path built from the wrong
// root, otherwise turns a check into a pass.
let imagesChecked = 0;
for (const [name, source] of Object.entries(pages)) {
  for (const [tag] of source.matchAll(/<img\b[^>]*>/g)) {
    imagesChecked += 1;
    const src = tag.match(/src="([^"]+)"/)?.[1];
    if (!/\balt="[^"]/.test(tag)) fail(`${name}: <img src="${src}"> has no alt text`);
    if (!/\bwidth="\d+"/.test(tag) || !/\bheight="\d+"/.test(tag)) {
      fail(`${name}: <img src="${src}"> has no intrinsic size, so the page will shift as it loads`);
    }
    if (src?.startsWith('/') && !existsSync(join(DIST, src.slice(1)))) {
      fail(`${name}: <img src="${src}"> does not exist`);
      continue;
    }

    // Declared size against real size: a stale width/height reserves the wrong
    // box and the page jumps when the image arrives. Re-cropping a screenshot
    // is exactly how they go stale.
    if (src?.endsWith('.webp')) {
      const real = webpSize(join(DIST, src.slice(1)));
      const declared = {
        width: Number(tag.match(/\bwidth="(\d+)"/)?.[1]),
        height: Number(tag.match(/\bheight="(\d+)"/)?.[1]),
      };
      if (real && (real.width !== declared.width || real.height !== declared.height)) {
        fail(`${name}: <img src="${src}"> declares ${declared.width}x${declared.height} but the file is ${real.width}x${real.height}`);
      }
      // The AVIF the <source> above offers is the same picture, or the page
      // jumps for the browsers that take it and not for the ones that do not.
      const avif = join(DIST, src.slice(1).replace(/\.webp$/, '.avif'));
      if (existsSync(avif)) {
        const size = avifSize(avif);
        if (!size) fail(`${name}: ${avif} carries no readable size`);
        else if (size.width !== declared.width || size.height !== declared.height) {
          fail(`${name}: the AVIF beside ${src} is ${size.width}x${size.height}, the page declares ${declared.width}x${declared.height}`);
        }
      }
    }
  }
}

if (imagesChecked < 8) fail(`only ${imagesChecked} <img> examined across the pages — the check read nothing`);

// -------------------------------------------------------------- links
let linksChecked = 0;
for (const [name, source] of Object.entries(pages)) {
  const ids = new Set([...source.matchAll(/\bid="([^"]+)"/g)].map((m) => m[1]));
  for (const [, href] of source.matchAll(/href="([^"]+)"/g)) {
    linksChecked += 1;
    if (href.startsWith('#')) {
      if (!ids.has(href.slice(1))) fail(`${name}: in-page link ${href} points at no element`);
    } else if (href.startsWith('/')) {
      const target = href.split('#')[0];
      const file = target.endsWith('/') ? `${target}index.html` : target;
      if (!existsSync(join(DIST, file.slice(1)))) fail(`${name}: link ${href} has no file behind it`);
    }
  }
}

if (linksChecked < 20) fail(`only ${linksChecked} href examined across the pages — the check read nothing`);

// -------------------------------------------------------------- no third party
// The site must *fetch* nothing from outside: it is the cheapest privacy
// guarantee and it is what keeps the CSP at `default-src 'none'`. Outbound
// <a href> links are navigation, not a request the visitor's browser makes.
for (const [name, source] of Object.entries(pages)) {
  for (const [tag] of source.matchAll(/<(?:link|script|img|source|iframe|video|audio)\b[^>]*>/g)) {
    // A <link rel="canonical"> names a URL, it does not fetch one.
    const rel = tag.match(/\brel="([^"]+)"/)?.[1];
    if (tag.startsWith('<link') && !/^(stylesheet|preload|prefetch|icon|apple-touch-icon|manifest|preconnect|dns-prefetch)$/.test(rel ?? '')) {
      continue;
    }
    const url = tag.match(/(?:src|srcset|href)="(https?:\/\/[^"]+)"/)?.[1];
    if (url) fail(`${name}: loads a third-party subresource: ${url}`);
  }
}

// -------------------------------------------------------------- the sitemap
// Generated by @astrojs/sitemap from the routes, so it cannot list a language
// the site does not build. What it *can* do is stop matching what robots.txt
// promises: the generator writes `sitemap-index.xml`, and the line in
// robots.txt naming it is written by hand.
{
  const promised = read('robots.txt').match(/^Sitemap:\s*(\S+)/m)?.[1];
  if (!promised) {
    fail('robots.txt names no sitemap');
  } else {
    const file = promised.replace(/^https?:\/\/[^/]+/, '').replace(/^\//, '');
    if (!existsSync(join(DIST, file))) fail(`robots.txt points at ${promised}, which is not built`);
  }

  // Every shipped language is in it. The generator makes that true; this makes
  // it *checked*, which a hand-written file never is — a missing entry there
  // leaves every check passing.
  const sitemap = readdirSync(DIST)
    .filter((f) => /^sitemap-\d+\.xml$/.test(f))
    .map((f) => read(f))
    .join('');
  for (const { code, path } of LANGUAGES) {
    if (!sitemap.includes(`${ORIGIN}${path}<`)) {
      fail(`the sitemap does not list ${path} (${code})`);
    }
  }
  if (sitemap.includes('/404')) fail('the sitemap lists /404, which is noindex');
  // Every entry names every translation, as the pages do.
  const alternates = (sitemap.match(/<xhtml:link\b/g) ?? []).length;
  const expected = LANGUAGES.length * LANGUAGES.length;
  if (alternates < expected) {
    fail(`the sitemap carries ${alternates} hreflang alternate(s), expected ${expected}`);
  }
}

// ------------------------------------------------------------- og:locale
// A link preview states the page's language, and each translation names the
// others; the English page said nothing and the others were English by default.
for (const { code } of LANGUAGES) {
  const file = code === 'en' ? 'index.html' : `${code}/index.html`;
  const page = pages[file];
  if (!page) continue;
  const stated = page.match(/property="og:locale" content="([a-z]{2}_[A-Z]{2})"/)?.[1];
  if (!stated?.startsWith(`${code}_`)) fail(`${file}: og:locale is ${stated ?? 'missing'}`);
  const others = (page.match(/property="og:locale:alternate"/g) ?? []).length;
  if (others !== LANGUAGES.length - 1) {
    fail(`${file}: ${others} og:locale:alternate, expected ${LANGUAGES.length - 1}`);
  }
}

// -------------------------------------------------------------- assets
const shots = readdirSync(join(DIST, 'assets/shots'));
if (!shots.length) fail('assets/shots is empty — run site/screenshots/run.sh');
for (const shot of shots) {
  if (!/\.(webp|avif)$/.test(shot)) {
    fail(`assets/shots/${shot} is neither webp nor avif; run.sh should have converted it`);
  }
}

// Every WebP has an AVIF beside it, and nothing the other way round. The two
// are encoded from the same PNG in one pass, so a lone file means a run that
// half-finished — and a missing AVIF is invisible in a browser that would have
// taken it, which is the whole point of the `<source>`.
for (const shot of shots.filter((s) => s.endsWith('.webp'))) {
  const avif = shot.replace(/\.webp$/, '.avif');
  if (!shots.includes(avif)) fail(`assets/shots/${avif} is missing — re-run site/screenshots/run.sh`);
}
for (const shot of shots.filter((s) => s.endsWith('.avif'))) {
  const webp = shot.replace(/\.avif$/, '.webp');
  if (!shots.includes(webp)) {
    fail(`assets/shots/${shot} has no webp fallback — re-run site/screenshots/run.sh`);
  }
}

// And nothing is shipped that no page shows. The pairing checks above run both
// ways between the two formats but never against the pages, so a capture the
// harness produces and the layout never uses was deployed, counted against the
// page weight of anyone who fetched it by URL, and kept current for nothing.
const referenced = new Set();
for (const source of Object.values(pages)) {
  for (const [, file] of source.matchAll(/assets\/shots\/([a-z0-9._-]+)/gi)) referenced.add(file);
}
for (const shot of shots) {
  if (!referenced.has(shot)) {
    fail(`assets/shots/${shot} is shipped but no page shows it — use it or stop capturing it`);
  }
}

// Every extension the site actually ships is one `serve.mjs` knows how to type.
// A missing entry serves the file with no content-type, and a browser may then
// refuse the `<source>` it would otherwise have taken — a difference visible
// only in preview, which is the one thing that server exists to prevent.
{
  const types = readFileSync(join(ROOT, 'serve.mjs'), 'utf-8').match(/const TYPES = \{([\s\S]*?)\}/)?.[1] ?? '';
  const known = new Set([...types.matchAll(/'(\.[a-z0-9]+)'/g)].map((m) => m[1]));
  const shipped = new Set(shots.map((s) => s.slice(s.lastIndexOf('.'))));
  for (const ext of shipped) {
    if (!known.has(ext)) fail(`serve.mjs has no MIME type for ${ext}`);
  }
}

// And every `<source>` in the page points at one that exists. A typo here is a
// broken image in exactly the browsers that support the better format.
let sourcesChecked = 0;
for (const match of index.matchAll(/<source srcset="([^"]+)"/g)) {
  sourcesChecked += 1;
  if (!existsSync(join(DIST, match[1].slice(1)))) fail(`<source> points at a missing ${match[1]}`);
}
if (sourcesChecked < 4) fail(`only ${sourcesChecked} <source> examined — the check read nothing`);

// -------------------------------------------------------------- icons
// The SVG favicon is the source of truth, but it is not enough on its own:
// older Safari ignores `type="image/svg+xml"`, and crawlers and unfurlers ask
// the origin root for /favicon.ico without reading the document at all. All
// three are rendered from the SVG by `node site/icons.mjs`; this fails when one
// was not regenerated, so the raster copies cannot silently fall behind the
// drawing they came from.
for (const file of ['assets/favicon-32.png', 'assets/apple-touch-icon.png']) {
  if (!existsSync(join(DIST, file))) fail(`${file} is missing — run node site/icons.mjs`);
  else if (!pngSize(join(DIST, file))) fail(`${file} is not a PNG`);
}

// iOS asks for 180x180 and does not read a `sizes` attribute, so that one is a
// constant rather than something the page declares.
const apple = pngSize(join(DIST, 'assets/apple-touch-icon.png'));
if (apple && (apple.width !== 180 || apple.height !== 180)) {
  fail(`assets/apple-touch-icon.png is ${apple.width}x${apple.height}, iOS asks for 180x180`);
}

// A `sizes` attribute is a promise to the browser, which picks an icon by it
// without ever opening the file. Compare it against the real pixels: announcing
// 32x32 over a 48x48 image is exactly the kind of mistake no visual review sees.
let iconSizesChecked = 0;
for (const [name, source] of Object.entries(pages)) {
  for (const [tag] of source.matchAll(/<link\b[^>]*\brel="(?:icon|apple-touch-icon)"[^>]*>/g)) {
    const href = tag.match(/\bhref="([^"]+)"/)?.[1];
    const declared = tag.match(/\bsizes="(\d+)x(\d+)"/);
    if (!href || !declared) continue;
    // Under `dist`, where the built page's paths resolve — read from the
    // repository root instead, no icon exists and nothing is ever compared.
    const size = pngSize(join(DIST, href.slice(1)));
    if (!size) {
      fail(`${name}: ${href} announces a size but is not a PNG that can be measured`);
      continue;
    }
    iconSizesChecked += 1;
    if (size.width !== Number(declared[1]) || size.height !== Number(declared[2])) {
      fail(`${name}: ${href} is announced ${declared[1]}x${declared[2]} but is ${size.width}x${size.height}`);
    }
  }
}
if (iconSizesChecked < Object.keys(pages).length) {
  fail(`only ${iconSizesChecked} icon size(s) measured across ${Object.keys(pages).length} pages`);
}

// The card a link preview renders: the page announces 1200x630, and a
// mismatched image is cropped or refused by the network that reads it.
{
  const og = pngSize(join(DIST, 'assets/og.png'));
  if (!og) fail('assets/og.png is missing or not a PNG');
  else {
    const declared = index.match(/property="og:image:width" content="(\d+)"[\s\S]*?property="og:image:height" content="(\d+)"/);
    if (!declared) fail('the page declares no og:image size');
    else if (og.width !== Number(declared[1]) || og.height !== Number(declared[2])) {
      fail(`assets/og.png is ${og.width}x${og.height}, the page declares ${declared[1]}x${declared[2]}`);
    }
  }
}

if (!existsSync(join(DIST, 'favicon.ico'))) {
  fail('favicon.ico is missing from the site root — run node site/icons.mjs');
} else {
  const ico = readFileSync(join(DIST, 'favicon.ico'));
  // Reserved word, type 1 (icon), at least one image.
  if (ico.readUInt16LE(0) !== 0 || ico.readUInt16LE(2) !== 1 || ico.readUInt16LE(4) < 1) {
    fail('favicon.ico is not a valid icon file');
  }
}

for (const [name, source] of Object.entries(pages)) {
  for (const reference of ['/assets/favicon.svg', '/assets/favicon-32.png', '/assets/apple-touch-icon.png']) {
    if (!source.includes(reference)) fail(`${name}: does not reference ${reference}`);
  }
}


// -------------------------------------------------------------- theme parity
// Read from the source rather than from `dist/`: Astro fingerprints the built
// stylesheet, so its name changes with its bytes and there is nothing stable
// to open, and the palettes are a property of what was written.
const css = readFileSync(join(ROOT, 'src/styles/site.css'), 'utf-8');
function tokensOf(pattern) {
  const block = css.match(pattern)?.[1] ?? '';
  return Object.fromEntries([...block.matchAll(/(--[a-z-]+):\s*([^;]+);/g)].map((m) => [m[1], m[2].trim()]));
}
// Dark is the default, and light is reached only by stamping
// `data-theme="light"`. A `prefers-color-scheme` block that redefined the
// palette would hand the default back to the visitor's system, which is the
// one decision here that must not be undone by accident.
if (/@media\s*\(prefers-color-scheme[^)]*\)\s*\{[^{]*\{[^}]*--bg:/.test(css)) {
  fail('site.css redefines the palette under prefers-color-scheme: dark is the default, light is a stamped choice');
}
const explicit = tokensOf(/:root\[data-theme="light"\]\s*\{([\s\S]*?)\}/);
if (Object.keys(explicit).length < 10) {
  fail(`the light palette reads ${Object.keys(explicit).length} token(s), so this check is reading nothing`);
}

// -------------------------------------------------------------- contrast
// WCAG 2.1 AA for normal text is 4.5:1. Measured rather than eyeballed: an
// accent at 4.06 looks perfectly fine.
function luminance(hex) {
  const channels = [1, 3, 5].map((i) => parseInt(hex.substr(i, 2), 16) / 255);
  const linear = channels.map((c) => (c <= 0.03928 ? c / 12.92 : ((c + 0.055) / 1.055) ** 2.4));
  return 0.2126 * linear[0] + 0.7152 * linear[1] + 0.0722 * linear[2];
}
function contrast(a, b) {
  const [hi, lo] = [luminance(a), luminance(b)].sort((x, y) => y - x);
  return (hi + 0.05) / (lo + 0.05);
}
const dark = tokensOf(/^:root\s*\{([\s\S]*?)\}/m);
// `--accent` is decorative only (borders, glows) and is exempt; the tokens that
// carry text are `--accent-text` (amber as text) and `--accent-ink` over both
// button fills. Both, since the primary button was flattened: its rest and
// hover states are now two flat colours rather than two stops of one gradient,
// so checking the darker one is no longer checking the worst case. `--focus`
// carries a state rather than text — the ring says where the keyboard is —
// and WCAG 1.4.11 asks 3:1 of it against everything it can land on.
const TEXT_PAIRS = [['--text', '--bg'], ['--text-soft', '--bg'], ['--text-muted', '--bg'], ['--text-muted', '--bg-card'], ['--accent-text', '--bg'], ['--accent-text', '--bg-card'], ['--accent-ink', '--accent-fill'], ['--accent-ink', '--accent-fill-hi']];
const STATE_PAIRS = [['--focus', '--bg', 3], ['--focus', '--bg-card', 3]];
for (const [label, palette] of [['dark', dark], ['light', explicit]]) {
  for (const [fg, bg, floor = 4.5] of [...TEXT_PAIRS, ...STATE_PAIRS]) {
    const a = palette[fg];
    const b = palette[bg];
    // A pair that cannot be measured is a token missing from the palette, not
    // a pass.
    if (!/^#[0-9a-f]{6}$/i.test(a ?? '') || !/^#[0-9a-f]{6}$/i.test(b ?? '')) {
      fail(`${label} theme: ${fg} on ${bg} cannot be measured (${a ?? 'unset'} on ${b ?? 'unset'})`);
      continue;
    }
    const value = contrast(a, b);
    if (value < floor) fail(`${label} theme: ${fg} on ${bg} is ${value.toFixed(2)}:1, WCAG needs ${floor}:1`);
  }
}

// ------------------------------------------------- immutable means fingerprinted
// `immutable` for a year is a promise that the bytes at this URL never change.
// Astro fingerprints what it builds, so `/_astro/*` can keep it; everything
// under `/assets/` is hand-placed and keeps its name. `screenshots/run.sh`
// overwrites `dashboard.avif` in place — four times in three days — and under
// the blanket rule a corrected screenshot never reached a returning visitor.
//
// The rule is the *name*, not the directory: a file added under `/assets/`
// tomorrow inherits the blanket rule unless it is carved out, and nothing would
// say so.
{
  const immutable = new Set();
  const bounded = new Set();
  let rule = null;
  for (const line of headers.split('\n')) {
    const path = line.match(/^(\/\S*)\s*$/)?.[1];
    if (path) {
      rule = path;
      continue;
    }
    if (!rule || !/cache-control/i.test(line)) continue;
    (/immutable/i.test(line) ? immutable : bounded).add(rule);
  }
  if (!immutable.size) fail('no `immutable` rule in _headers — the parser read nothing');

  const fingerprinted = /[.-][0-9A-Za-z_-]{8,}\.[a-z0-9]+$/;
  const served = [];
  const walk = (dir) => {
    for (const entry of readdirSync(join(DIST, dir), { withFileTypes: true })) {
      const at = `${dir}/${entry.name}`;
      if (entry.isDirectory()) walk(at);
      else served.push(at);
    }
  };
  walk('assets');
  if (served.length < 5) fail(`only ${served.length} file(s) found under /assets — the walk is wrong`);

  for (const file of served) {
    const url = `/${file}`;
    if (fingerprinted.test(url)) continue;
    const covered = [...bounded].some(
      (pattern) => pattern === url || (pattern.endsWith('*') && url.startsWith(pattern.slice(0, -1))),
    );
    if (!covered) {
      fail(
        `${url} carries no fingerprint and no bounded Cache-Control, so it inherits ` +
          '`immutable` for a year — replacing it in place would reach nobody',
      );
    }
  }
}

if (failures.length) {
  console.error(`\n${failures.length} problem(s):\n`);
  for (const problem of failures) console.error(`  - ${problem}`);
  process.exit(1);
}
console.log(`site checks passed (origin: ${ORIGIN})`);
