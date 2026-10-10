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
import { readFileSync, existsSync, readdirSync, statSync } from 'node:fs';
import { createHash } from 'node:crypto';
import { join, relative } from 'node:path';
import { fileURLToPath } from 'node:url';

const ROOT = fileURLToPath(new URL('.', import.meta.url));

/**
 * Everything here is checked against `dist/`, which is what Cloudflare serves.
 * Checking the sources would test the intention, and this tests the artefact:
 * the distinction matters for a CSP hash, a fingerprinted stylesheet, or an
 * image that exists in `public/` and never reached the build.
 */
const DIST = join(ROOT, 'dist');
const read = (file) => readFileSync(join(DIST, file), 'utf-8');

if (!existsSync(DIST)) {
  console.error('site/dist is missing: run `npm run build` in site/ first.');
  process.exit(1);
}

// The one list of languages, read from the source the pages are built from:
// a copy here would miss a language added there.
const { LANGUAGES } = await import('./src/i18n/languages.ts');

/**
 * Where a language's copy of a built file lands, from the language's `path`:
 * English at the root, every other language under its prefix.
 */
const builtAs = (path, file) => `${path.slice(1)}${file}`;


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
 * failed halfway leaves behind, and the AVIF is the file most browsers take.
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
// Read before the origin check, which scans it, and only when it exists: a
// missing file is a failure the llms.txt section names, not a crash here.
const llms = existsSync(join(DIST, 'llms.txt')) ? read('llms.txt') : null;


// -------------------------------------------------------------- one origin
// The canonical, the sitemap, robots.txt and llms.txt state the site's origin.
// A partial rename, one updated and another forgotten, is the failure mode.
const origins = new Set();
const securityTxt = existsSync(join(DIST, '.well-known/security.txt')) ? read('.well-known/security.txt') : '';
if (!securityTxt) fail('.well-known/security.txt is missing from the build');
// Past its date, the file says the contact in it is no longer watched.
const securityExpires = Date.parse(securityTxt.match(/^Expires:\s*(\S+)/m)?.[1] ?? '');
if (!(securityExpires > Date.now())) fail('.well-known/security.txt has expired or states no Expires: renew it');
for (const source of [index, notFound, headers, read('robots.txt'), read('sitemap-index.xml'), llms ?? '', securityTxt]) {
  for (const [, origin] of source.matchAll(/https?:\/\/([a-z0-9.-]+)/gi)) origins.add(origin);
}
// `localhost` appears in the install instructions as prose, not as a host the
// site talks to, and `_headers` names the Web Analytics beacon's host. The rest
// are well-known vocabulary and outbound links.
const EXTERNAL = /(github|schema\.org|gnu\.org|ogp\.me|sitemaps\.org|w3\.org|localhost|127\.0\.0\.1|cloudflareinsights\.com)/;
const own = [...origins].filter((o) => !EXTERNAL.test(o));
if (own.length > 1) fail(`several site origins in use, pick one: ${own.join(', ')}`);
const ORIGIN = own[0];

// ------------------------------------------------------------ translations
// Astro renders the four pages from one set of components, so a translation
// cannot be *stale*: there is no second copy of the page to fall behind.
// What is possible is a catalogue that has drifted from the keys the
// components ask for, so that is what this checks: same keys as English, none
// empty, and a built page at the path the switcher and the sitemap promise.
const catalogue = (code) =>
  JSON.parse(readFileSync(join(ROOT, `src/i18n/${code}.json`), 'utf-8'));

const english = catalogue('en');
/* Every page but the landing, under each language's prefix. */
const SUBPAGES = ['how/index.html', 'api/index.html'];
for (const { code, path } of LANGUAGES) {
  const file = builtAs(path, 'index.html');

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
    fail(`${file} was not built: run \`npm run build\` in site/`);
    continue;
  }
  pages[file] = read(file);

  /* The other pages are checked exactly as the landing is. Left out, most of
     the site would ship with nothing looking at its links, its labels or the
     English phrases that escape a catalogue. */
  for (const other of SUBPAGES) {
    const built = builtAs(path, other);
    if (!existsSync(join(DIST, built))) {
      fail(`${built} was not built: run \`npm run build\` in site/`);
    } else {
      pages[built] = read(built);
    }
  }

  // The not-found page Cloudflare serves for a miss under this language's
  // prefix, the English one at the root. Astro writes a translated route as a
  // directory, and `astro.config.mjs` moves it to where the lookup finds it.
  const notFoundPage = builtAs(path, '404.html');
  if (!existsSync(join(DIST, notFoundPage))) {
    fail(`${notFoundPage} was not built, so a miss under ${path} is answered in English`);
  } else {
    pages[notFoundPage] = read(notFoundPage);
    if (!pages[notFoundPage].includes(`<html lang="${code}"`)) fail(`${notFoundPage} is not in ${code}`);
  }

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
// three other pages. Every label, alternative text, title and placeholder a
// translated page carries has to differ from the ones of the same page in
// English, and no translated page carries a phrase of `ESCAPED`, English text
// written outside an attribute.
const labelsOf = (html) =>
  new Set([...html.matchAll(/\b(?:aria-label|alt|title|placeholder|data-copied|data-failed(?:-mac)?)="([^"]*)"/g)].map((m) => m[1]));
const englishLabels = Object.fromEntries(
  ['index.html', ...SUBPAGES].map((file) => [file, labelsOf(pages[builtAs('/', file)] ?? '')]),
);
for (const [file, labels] of Object.entries(englishLabels)) {
  if (labels.size < 4) fail(`only ${labels.size} label(s) on the English ${file}: the guard read nothing`);
}
const ESCAPED = ['one compose up', 'Global dry-run', 'Batch limit', '// before', 'Press Ctrl+C', 'Not affiliated'];
for (const { code, path } of LANGUAGES) {
  if (code === 'en') continue;
  for (const other of ['index.html', ...SUBPAGES]) {
    const file = builtAs(path, other);
    const page = pages[file];
    if (!page) continue;
    for (const label of labelsOf(page)) {
      if (englishLabels[other].has(label)) fail(`${file} announces "${label}" in English`);
    }
    for (const phrase of ESCAPED) {
      if (page.includes(phrase)) fail(`${file} still says "${phrase}" in English`);
    }
  }
}

// ------------------------------------------------------ the sitemap agrees
// The `hreflang` set is declared twice by two different mechanisms: the pages
// write it in their `<head>` from `pathFor`, and the sitemap integration
// writes it from the routes. Google reads both, so they have to say the same
// thing, and nothing but this compares them. A missing `x-default` on one
// side is exactly the kind of drift that shows up months later in Search
// Console and never in a build.
const sitemapIndex = read('sitemap-index.xml');
const sitemapFiles = [...sitemapIndex.matchAll(/<loc>[^<]*\/([^/<]+\.xml)<\/loc>/g)].map((m) => m[1]);
if (!sitemapFiles.length) fail('sitemap-index.xml names no sitemap');

const robotsSitemap = read('robots.txt').match(/^Sitemap:\s*(\S+)/m);
if (!robotsSitemap) {
  fail('robots.txt names no sitemap');
} else if (!robotsSitemap[1].startsWith(`https://${ORIGIN}/`) || !robotsSitemap[1].endsWith('sitemap-index.xml')) {
  fail(`robots.txt points at ${robotsSitemap[1]}, which is not this site's sitemap index`);
} else if (!existsSync(join(DIST, robotsSitemap[1].replace(`https://${ORIGIN}/`, '')))) {
  // The line in robots.txt is written by hand, the index by the generator.
  fail(`robots.txt points at ${robotsSitemap[1]}, which is not built`);
}

const inSitemap = new Set();
let comparedPages = 0;
for (const file of sitemapFiles) {
  const xml = read(file);
  for (const entry of xml.matchAll(/<url>\s*<loc>([^<]+)<\/loc>([\s\S]*?)<\/url>/g)) {
    const [, loc, rest] = entry;
    inSitemap.add(loc);

    const tail = loc.replace(`https://${ORIGIN}/`, '');
    const file = tail === '' ? 'index.html' : `${tail.replace(/\/$/, '')}/index.html`;
    const page = pages[file];
    if (!page) {
      fail(`the sitemap lists ${loc}, which was not built as ${file}`);
      continue;
    }
    comparedPages += 1;

    const fromSitemap = new Set([...rest.matchAll(/hreflang="([^"]+)"/g)].map((m) => m[1]));
    const fromPage = new Set([...page.matchAll(/<link rel="alternate" hreflang="([^"]+)"/g)].map((m) => m[1]));
    const missing = [...fromPage].filter((lang) => !fromSitemap.has(lang));
    const extra = [...fromSitemap].filter((lang) => !fromPage.has(lang));
    if (missing.length) fail(`${file}: the sitemap omits hreflang ${missing.join(', ')}, which the page declares`);
    if (extra.length) fail(`${file}: the sitemap declares hreflang ${extra.join(', ')}, which the page does not`);

    // Exactly one `x-default` per entry: it names the one fallback page.
    const defaults = [...rest.matchAll(/hreflang="x-default" href="([^"]+)"/g)].map((m) => m[1]);
    if (defaults.length !== 1) fail(`${file}: the sitemap declares ${defaults.length} x-default entries, expected one`);
  }
}

// Every built page is offered, and the count is the guard on the guard.
for (const file of Object.keys(pages)) {
  if (file.endsWith('404.html')) continue;
  const path = file === 'index.html' ? '' : file.replace(/index\.html$/, '');
  if (!inSitemap.has(`https://${ORIGIN}/${path}`)) fail(`${file} was built but is not in the sitemap`);
}
if (inSitemap.has(`https://${ORIGIN}/404`) || inSitemap.has(`https://${ORIGIN}/404/`)) {
  fail('the sitemap lists /404, the one page that tells crawlers to go away');
}
if (comparedPages < 12) {
  fail(`compared ${comparedPages} page(s) against the sitemap, so this check is reading almost nothing`);
}

// -------------------------------------------- every page revalidates
// Astro fingerprints what it builds and does not fingerprint the pages, so an
// HTML file cached for any length of time is a page a deploy cannot take back.
// Each built page needs its own rule in `_headers`, so a page added without one
// fails the build instead of going stale quietly.
for (const file of Object.keys(pages)) {
  const path = file === 'index.html' ? '/' : `/${file.replace(/index\.html$/, '')}`;
  const rule = new RegExp(`^${path.replace(/[.*+?^${}()|[\]\\]/g, '\\$&')}\\s*\\n\\s+Cache-Control:[^\\n]*must-revalidate`, 'm');
  if (!rule.test(headers)) {
    fail(`_headers has no revalidating Cache-Control for ${path}, so that page can be served stale`);
  }
}

// --------------------------------------------- markup that reached the page
// A catalogue value carrying a tag has to be rendered with `set:html`. Slotted
// as text it is escaped, and the page shows `<em>26 languages.</em>` in full.
// The mistake sits in a single component, and no other check sees it.
for (const [file, page] of Object.entries(pages)) {
  // Attributes included: their quotes are escaped to `&quot;` as well.
  const escaped = page.match(/&lt;\/?(em|strong|b|code|span|br|a|kbd)\b(?:[^&]|&quot;|&#39;|&amp;){0,80}?&gt;/);
  if (escaped) {
    fail(`${file} shows the markup ${escaped[0]} as text: that value needs set:html, or the tag does not belong in the catalogue`);
  }
}

// --------------------------------------------------- the application's words
// A new user looks in the application for the words the site gave them: the
// setting to turn off, the mode it shows, the screen that holds exceptions.
// Another name for one switch sends them looking for a setting nothing shows.
// A term `within` a sentence is a screen or a scope the sentence names.
const TERMS = [
  ['hero.board.mode', 'ModeDryRunShort'],
  ['safety.b', 'SettingGlobalDryRun'],
  ['safety.b.2', 'SettingBatchLimit'],
  ['safety.b.3', 'SettingConfirmationThreshold'],
  ['features.k.3', 'Overrides'],
  ['api.step.1', 'Applications', 'within'],
  ['api.step.1', 'ScopeOperate', 'within'],
  ['api.step.1', 'ScopeWrite', 'within'],
  ['api.step.3', 'ApiReference', 'within'],
  ['api.step.3', 'Scope', 'within'],
];
for (const { code } of LANGUAGES) {
  const app = JSON.parse(readFileSync(join(ROOT, `../backend/locales/${code}.json`), 'utf-8'));
  const site = JSON.parse(readFileSync(join(ROOT, `src/i18n/${code}.json`), 'utf-8'));
  for (const [siteKey, appKey, within] of TERMS) {
    const said = (site[siteKey] ?? '').toLowerCase();
    const term = (app[appKey] ?? '').toLowerCase();
    if (!term || (within ? !said.includes(term) : said !== term)) {
      fail(`src/i18n/${code}.json: ${siteKey} reads "${site[siteKey]}", the application says "${app[appKey]}"`);
    }
  }
}

// ------------------------------------------------------- one origin, every page
// The origin above is read off the landing and the files beside it. Every
// other built page names it too, and no other host of the site's own.
for (const [file, page] of Object.entries(pages)) {
  for (const [, origin] of page.matchAll(/https?:\/\/([a-z0-9.-]+)/gi)) {
    if (!EXTERNAL.test(origin) && origin !== ORIGIN) fail(`${file} names ${origin}, while the site is ${ORIGIN}`);
  }
}

// ---------------------------------------------------- written into a component
// Text a reader should get in their language comes from the catalogues. Written
// into a component, it ships in English on the three translated pages. What is
// let through is no language: the name, and a category and a file shown as
// data.
const NOT_LANGUAGE = new Set(['Routarr', 'anime', 'docker-compose.yml']);
let templatesRead = 0;
for (const entry of readdirSync(join(ROOT, 'src'), { recursive: true, withFileTypes: true })) {
  if (!entry.name.endsWith('.astro')) continue;
  templatesRead++;
  const file = join(entry.parentPath ?? entry.path, entry.name);
  let template = readFileSync(file, 'utf-8').replace(/^---[\s\S]*?\n---/, '');
  template = template
    .replace(/<!--[\s\S]*?-->/g, '')
    .replace(/<(script|style|pre|code|svg)\b[\s\S]*?<\/\1>/g, '');
  let outside = '';
  let depth = 0;
  for (const c of template) {
    if (c === '{') depth++;
    else if (c === '}') depth--;
    else if (depth === 0) outside += c;
  }
  const words = outside
    .replace(/<[^>]*>/g, ' ')
    .replace(/&[a-z]+;/g, ' ')
    .split(/\s+/)
    .filter((word) => /[A-Za-z]{2,}/.test(word) && !NOT_LANGUAGE.has(word));
  if (words.length) fail(`${relative(ROOT, file)} writes text no catalogue translates: ${words.slice(0, 6).join(' ')}`);
}
if (templatesRead < 10) fail(`read ${templatesRead} template(s), so the written-text check is reading almost nothing`);

// ------------------------------------------------------------ first-run command
// The command that prints the generated key is the first-run screen's, which
// the image smoke test runs, restated in the README, llms.txt and the install
// section. A copy that drifts sends a new user to a file that is not there.
const readKey = readFileSync(join(ROOT, '../frontend/src/components/ApiKeyGate.svelte'), 'utf-8')
  .match(/const READ_KEY_COMMAND = '([^']+)'/)?.[1];
if (!readKey) {
  fail('READ_KEY_COMMAND cannot be read out of frontend/src/components/ApiKeyGate.svelte');
} else {
  for (const [file, text] of [
    ['README.md', readFileSync(join(ROOT, '../README.md'), 'utf-8')],
    ['public/llms.txt', llms ?? ''],
    ['src/components/sections/Start.astro', readFileSync(join(ROOT, 'src/components/sections/Start.astro'), 'utf-8')],
  ]) {
    const stated = [...text.matchAll(/docker exec [^\s'"`,]+ cat [^\s'"`,]+/g)].map((m) => m[0]);
    if (!stated.length || stated.some((command) => command !== readKey)) {
      fail(`${file} states the first-run command as ${stated.join(', ') || 'nothing'}, the application as ${readKey}`);
    }
  }
}

// ------------------------------------------------------- keys nobody asks for
// A catalogue grows by addition and never by subtraction unless something
// looks: a key that outlives the section using it is four strings a translator
// is asked to keep. A key is referenced literally by a component.
{
  const sources = readdirSync(join(ROOT, 'src'), { recursive: true, withFileTypes: true })
    .filter((e) => e.isFile() && /\.(astro|ts)$/.test(e.name))
    .map((e) => readFileSync(join(e.parentPath ?? e.path, e.name), 'utf-8'))
    .join('\n');
  // The reference builds its keys from the contract's own names
  // (`src/contract.ts`), so they are read here from this checkout's contract:
  // each operation and group of it has a sentence, and a key naming none is
  // left behind. A release's contract is part of this one, since an
  // operation is never removed.
  const contract = JSON.parse(readFileSync(join(ROOT, '../backend/openapi/v1.json'), 'utf-8'));
  const operations = Object.values(contract.paths).flatMap((methods) =>
    Object.values(methods).map((operation) => operation.operationId),
  );
  const built = new Set([
    ...operations.map((id) => `api.ref.op.${id}`),
    ...(contract.tags ?? []).flatMap(({ name }) => [`api.ref.tag.${name}`, `api.ref.tag.${name}.note`]),
    ...['path', 'query', 'header'].map((where) => `api.ref.in.${where}`),
  ]);
  for (const key of built) {
    if (!(key in english)) fail(`src/i18n/en.json has no ${key}, which the API reference builds from the contract`);
  }
  const unused = Object.keys(english).filter((key) => {
    if (built.has(key)) return false;
    return !sources.includes(`'${key}'`) && !sources.includes(`"${key}"`);
  });
  if (unused.length) {
    fail(`src/i18n/en.json has ${unused.length} key(s) no component references: ${unused.slice(0, 4).join(', ')}${unused.length > 4 ? '…' : ''}`);
  }
}

// ------------------------------------------------------------ French spacing
// French puts a space before `?`, `!`, `:`, `;`, `»` and `%` and after `«`. A
// plain one lets the mark wrap alone onto the next line on a phone.
for (const [key, value] of Object.entries(catalogue('fr'))) {
  const loose = String(value).match(/\S ([?!:;»%])|« /);
  if (loose) fail(`src/i18n/fr.json: ${key} breaks before or after "${loose[0].trim()}", use a no-break space`);
}

// ------------------------------------------------- entities in a catalogue
// A catalogue value is a string, not markup. An `&amp;` written there is
// double-escaped the moment it goes through a template that escapes, and the
// page then shows the entity itself: `amd64 &amp; arm64` renders as written.
for (const { code } of LANGUAGES) {
  const dictionary = catalogue(code);
  for (const [key, value] of Object.entries(dictionary)) {
    const entity = String(value).match(/&(amp|lt|gt|quot|#\d+);/);
    if (entity && !/<[a-z]/i.test(String(value))) {
      fail(`src/i18n/${code}.json: ${key} carries the entity ${entity[0]} in a value that is not markup`);
    }
  }
}

// ---------------------------------------------------------- words in CSS
// A word a stylesheet prints through `content` never reaches a catalogue, so it
// reads in English on all four pages. The check above cannot see it, since the
// key it would need does not exist. Symbols are fine there, letters are not.
// CSS escapes are removed first: `\2212` is a minus sign, not text.
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
  fail(`read ${printed} CSS content string(s) in ${stylesheets.length} stylesheet(s): the check is reading nothing`);
}

// ----------------------------------------------------------- version
// The page states which version it describes, anywhere a visitor reads it: the
// one `backend/Cargo.toml` declares, and the JSON-LD says the same. Production
// is built at a release's tag, which `site.yml` names in
// `ROUTARR_SITE_RELEASE`: a checkout whose crate names another version would
// deploy a page describing a version nobody can pull.
const cargo = readFileSync(join(ROOT, '../backend/Cargo.toml'), 'utf-8');
const version = cargo.match(/^version = "([^"]+)"/m)?.[1];
if (!version) fail('could not read the version from Cargo.toml');
else {
  if (!index.includes(`"softwareVersion": "${version}"`)) {
    fail(`index.html: softwareVersion is not ${version}, the version backend/Cargo.toml declares`);
  }
  if (!index.includes(`v${version}`)) {
    fail(`index.html: the page states no version, and backend/Cargo.toml declares ${version}`);
  }
  const release = process.env.ROUTARR_SITE_RELEASE;
  if (release !== undefined && release !== `v${version}`) {
    fail(`the build is for the release ${release || '(none named)'}, and this checkout declares ${version}`);
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
if (!index.includes('id="lang-hint"')) fail('index.html: #lang-hint is missing: site.js builds the banner into it');
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
    fail(`the inline script's hash is not in _headers: the page would render unstyled and dead.\n    expected: ${digest}`);
  }
}
// Every page, and every inline script whatever its `type`: a component's
// `<script>` short enough for Astro to inline arrives as `type="module"`, and
// the CSP blocks it without a word. Structured data is not a script.
for (const [file, page] of Object.entries(pages)) {
  for (const [, script] of page.matchAll(/<script(?![^>]*\bsrc=)(?![^>]*application\/ld\+json)[^>]*>([\s\S]*?)<\/script>/g)) {
    if (!headers.includes(`sha256-${createHash('sha256').update(script).digest('base64')}`)) {
      fail(`${file} carries an inline script whose hash is not in _headers, so the CSP blocks it`);
    }
  }
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
// The structured data states the page's language, so its description is in it:
// an English sentence declared French is what a search engine shows a French
// reader.
for (const [file, source] of Object.entries(pages)) {
  const block = source.match(/<script type="application\/ld\+json">([\s\S]*?)<\/script>/)?.[1];
  if (!block) continue;
  const data = JSON.parse(block);
  const lang = source.match(/<html lang="([a-z]+)"/)?.[1];
  const meta = source.match(/<meta name="description" content="([^"]*)"/)?.[1]
    ?.replace(/&#39;/g, "'").replace(/&quot;/g, '"').replace(/&amp;/g, '&');
  if (data.inLanguage !== lang) fail(`${file}: the structured data says ${data.inLanguage}, the page ${lang}`);
  if (data.description !== meta) fail(`${file}: the structured data describes the page in other words than its own description`);
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

const imagesOnPage = Object.values(pages).reduce((n, html) => n + (html.match(/<img /g) || []).length, 0);
if (imagesOnPage && imagesChecked < imagesOnPage) {
  fail(`${imagesOnPage} <img> on the pages and only ${imagesChecked} examined: the check read past some`);
}

// -------------------------------------------------------------- links
let linksChecked = 0;
for (const [name, source] of Object.entries(pages)) {
  const ids = new Set([...source.matchAll(/\bid="([^"]+)"/g)].map((m) => m[1]));
  for (const [, href] of source.matchAll(/href="([^"]+)"/g)) {
    linksChecked += 1;
    if (href.startsWith('#')) {
      if (!ids.has(href.slice(1))) fail(`${name}: in-page link ${href} points at no element`);
    } else if (href.startsWith('/')) {
      const [target, fragment] = href.split('#');
      const file = target.endsWith('/') ? `${target}index.html` : target;
      if (!existsSync(join(DIST, file.slice(1)))) {
        fail(`${name}: link ${href} has no file behind it`);
      } else if (fragment !== undefined && !(pages[file.slice(1)] ?? read(file.slice(1))).includes(`id="${fragment}"`)) {
        fail(`${name}: link ${href} points at no element of ${file}`);
      }
    }
  }
}

// The landing's last card names six subjects of the detail page. Six links to
// its top would name places the reader is never taken to.
for (const [name, source] of Object.entries(pages)) {
  const list = source.match(/<ul class="more-list">([\s\S]*?)<\/ul>/)?.[1];
  if (!list) continue;
  const targets = [...list.matchAll(/href="([^"]+)"/g)].map((m) => m[1]);
  if (new Set(targets).size !== targets.length) fail(`${name}: the subjects of the "More" card share a destination`);
}

if (linksChecked < 20) fail(`only ${linksChecked} href examined across the pages: the check read nothing`);

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

// -------------------------------------------------------------- llms.txt
// Written by hand, so a link in it can name a page the build never produced.
// The convention (llmstxt.org) fixes how it opens: an H1 naming the project,
// then a blockquote summing it up. A page is linked by its trailing slash,
// since the bare directory answers with a redirect.
if (llms === null) {
  fail('dist/llms.txt is missing: the build copies it from site/public/llms.txt');
} else {
  const [first = '', second = ''] = llms.split('\n').filter((line) => line.trim());
  if (!first.startsWith('# ')) fail(`llms.txt opens with "${first}" instead of an H1 naming the project`);
  if (!second.startsWith('> ')) fail(`llms.txt follows its first line with "${second}" instead of a blockquote summary`);

  let linked = 0;
  for (const [written] of llms.matchAll(/https?:\/\/[^\s()<>[\]"'`]+/g)) {
    const address = written.replace(/[.,:!?]+$/, '');
    const url = new URL(address);
    if (url.hostname !== ORIGIN) continue;
    linked += 1;
    const file = url.pathname.endsWith('/') ? `${url.pathname}index.html` : url.pathname;
    const entry = statSync(join(DIST, file), { throwIfNoEntry: false });
    if (entry?.isDirectory()) {
      fail(`llms.txt links to ${address}, a directory that answers with a redirect: end the path with a slash`);
    } else if (!entry?.isFile()) {
      fail(`llms.txt links to ${address}, and nothing is built at dist${file}`);
    }
  }
  // The count is the guard on the guard: a pattern that stops matching the
  // links passes having checked none.
  if (!linked) fail(`llms.txt links to no page on ${ORIGIN}, so the link check read nothing`);
}

// ------------------------------------------------------------- og:locale
// A link preview states the page's language, and each translation names the
// others. A page that omits `og:locale` is taken for `en_US`, whatever its
// language.
for (const { code, path } of LANGUAGES) {
  const file = builtAs(path, 'index.html');
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
// `screenshots/run.sh` writes its captures outside `public/`, so the directory
// holds only what a page takes in. What must never happen is shipping one
// nothing shows.
const shots = existsSync(join(DIST, 'assets/shots'))
  ? readdirSync(join(DIST, 'assets/shots'))
  : [];
for (const shot of shots) {
  if (!/\.(webp|avif)$/.test(shot)) {
    fail(`assets/shots/${shot} is neither webp nor avif; run.sh should have converted it`);
  }
}

// Every WebP has an AVIF beside it, and every AVIF a WebP. The two are encoded
// from the same PNG in one pass, so a lone file means a run that half-finished.
// A missing AVIF is invisible in a browser that would have taken it, and that
// browser is the whole point of the `<source>`.
for (const shot of shots.filter((s) => s.endsWith('.webp'))) {
  const avif = shot.replace(/\.webp$/, '.avif');
  if (!shots.includes(avif)) fail(`assets/shots/${avif} is missing: re-run site/screenshots/run.sh`);
}
for (const shot of shots.filter((s) => s.endsWith('.avif'))) {
  const webp = shot.replace(/\.avif$/, '.webp');
  if (!shots.includes(webp)) {
    fail(`assets/shots/${shot} has no webp fallback: re-run site/screenshots/run.sh`);
  }
}

// And nothing is shipped that no page shows. The pairing checks above run both
// ways between the two formats but never against the pages. Without this, a
// capture the harness produces and the layout never uses would ship, weigh on
// anyone who fetches it by URL, and be kept current for nothing.
const referenced = new Set();
for (const source of Object.values(pages)) {
  for (const [, file] of source.matchAll(/assets\/shots\/([a-z0-9._-]+)/gi)) referenced.add(file);
}
for (const shot of shots) {
  if (!referenced.has(shot)) {
    fail(`assets/shots/${shot} is shipped but no page shows it: use it or stop capturing it`);
  }
}

// Every extension the site actually ships is one `serve.mjs` knows how to type.
// A missing entry serves the file with no content-type, and a browser may then
// refuse the `<source>` it would otherwise have taken. The preview then differs
// from production, which is the one thing that server exists to prevent.
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
const sourcesOnPage = (index.match(/<source /g) || []).length;
if (sourcesOnPage && sourcesChecked < sourcesOnPage) {
  fail(`${sourcesOnPage} <source> on the page and only ${sourcesChecked} examined: the check read past some`);
}

// -------------------------------------------------------------- icons
// The SVG favicon is the source of truth, but it is not enough on its own:
// older Safari ignores `type="image/svg+xml"`, and crawlers and unfurlers ask
// the origin root for /favicon.ico without reading the document at all. All
// three are rendered from the SVG by `node site/icons.mjs`. This fails when one
// is missing or is not the format it claims, but it cannot see a copy older
// than the drawing: an edit to the SVG reruns the script.
for (const file of ['assets/favicon-32.png', 'assets/apple-touch-icon.png']) {
  if (!existsSync(join(DIST, file))) fail(`${file} is missing: run node site/icons.mjs`);
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
    // Under `dist`, where the built page's paths resolve. Read from the
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
  fail('favicon.ico is missing from the site root: run node site/icons.mjs');
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
// Light is the default, and dark is reached only by stamping
// `data-theme="dark"`. A `prefers-color-scheme` block that redefined the
// palette would hand the default back to the visitor's system, which is the
// one decision here that must not be undone by accident.
if (/@media\s*\(prefers-color-scheme[^)]*\)\s*\{[^{]*\{[^}]*--bg:/.test(css)) {
  fail('site.css redefines the palette under prefers-color-scheme: light is the default, dark is a stamped choice');
}
const explicit = tokensOf(/:root\[data-theme="dark"\]\s*\{([\s\S]*?)\}/);
if (Object.keys(explicit).length < 10) {
  fail(`the dark palette reads ${Object.keys(explicit).length} token(s), so this check is reading nothing`);
}
// A colour the light palette sets and the dark one leaves alone shows its light
// value on a dark page, which the contrast pairs below may never measure.
const lightPalette = tokensOf(/^:root\s*\{([\s\S]*?)\}/m);
const colours = (palette) =>
  Object.keys(palette).filter((token) => /^(#|rgb|hsl|oklch|color-mix)/.test(palette[token]));
for (const token of colours(lightPalette)) {
  if (!(token in explicit)) fail(`site.css: the dark palette leaves ${token} at its light value`);
}
for (const token of colours(explicit)) {
  if (!(token in lightPalette)) fail(`site.css: the dark palette sets ${token}, which the light one never defines`);
}

// -------------------------------------------------------------- tokens that exist
// A `var()` naming a token defined nowhere makes its declaration invalid, which
// the browser drops without a word: a hover that changes nothing.
const definedTokens = new Set([...css.matchAll(/(--[a-z0-9-]+)\s*:/g)].map((m) => m[1]));
const undefinedTokens = [...new Set([...css.matchAll(/var\(\s*(--[a-z0-9-]+)/g)].map((m) => m[1]))]
  .filter((token) => !definedTokens.has(token));
if (undefinedTokens.length) fail(`site.css reads tokens it never defines: ${undefinedTokens.join(', ')}`);

// -------------------------------------------------------------- contrast
// WCAG 2.1 AA for normal text is 4.5:1. Measured rather than eyeballed: an
// accent just under the floor looks perfectly fine.
function luminance(hex) {
  const channels = [1, 3, 5].map((i) => parseInt(hex.substr(i, 2), 16) / 255);
  const linear = channels.map((c) => (c <= 0.03928 ? c / 12.92 : ((c + 0.055) / 1.055) ** 2.4));
  return 0.2126 * linear[0] + 0.7152 * linear[1] + 0.0722 * linear[2];
}
function contrast(a, b) {
  const [hi, lo] = [luminance(a), luminance(b)].sort((x, y) => y - x);
  return (hi + 0.05) / (lo + 0.05);
}
// The bare `:root` block is the light default, `explicit` the stamped dark one.
const light = tokensOf(/^:root\s*\{([\s\S]*?)\}/m);
// `--accent` is decorative only (borders, glows) and is exempt. The tokens that
// carry text are `--accent-text` (amber as text) and `--accent-ink` over both
// button fills: the primary button's rest and hover states are two flat
// colours, and either can be the worst case. `--focus` carries a state rather
// than text (the ring says where the keyboard is), and WCAG 1.4.11 asks 3:1 of
// it against everything it can land on.
const TEXT_PAIRS = [['--text', '--bg'], ['--text-soft', '--bg'], ['--text-muted', '--bg'], ['--text-muted', '--bg-card'], ['--accent-text', '--bg'], ['--accent-text', '--bg-card'], ['--accent-ink', '--accent-fill'], ['--accent-ink', '--accent-fill-hi']];
const STATE_PAIRS = [['--focus', '--bg', 3], ['--focus', '--bg-card', 3]];
for (const [label, palette] of [['light', light], ['dark', explicit]]) {
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

// Text on a ground the accent tints, which the pairs above cannot see: the
// hero at its gradient's peak, which axe cannot measure through the gradient
// and the grid laid over it, and the plan's chips over its header and its rows.
// Each tint is read out of its own rule.
function tinted(ground, tint, share) {
  const channels = (hex) => [1, 3, 5].map((i) => parseInt(hex.substr(i, 2), 16));
  const [g, t] = [channels(ground), channels(tint)];
  return `#${g.map((c, i) => Math.round(c * (1 - share) + t[i] * share).toString(16).padStart(2, '0')).join('')}`;
}
function tintOf(rule) {
  const block = css.match(rule)?.[1] ?? '';
  return Math.max(...[...block.matchAll(/var\(--accent\) (\d+)%/g)].map((m) => Number(m[1])));
}
const TINTED = [
  ['the hero', tintOf(/^\.hero \{([\s\S]*?)^\}/m), ['--bg'], ['--text', '--text-soft', '--accent-text']],
  ['a chip', tintOf(/^\.chip\.is-go \{([^}]*)\}/m), ['--bg-raised', '--bg-card'], ['--accent-text']],
  ['the match mode in force', tintOf(/^\.rule-mode \.on \{([^}]*)\}/m), ['--bg-card'], ['--accent-text']],
];
for (const [what, tint, grounds, texts] of TINTED) {
  if (!Number.isFinite(tint)) {
    fail(`the ground of ${what} cannot be read out of its rule`);
    continue;
  }
  for (const [label, palette] of [['light', light], ['dark', explicit]]) {
    for (const base of grounds) {
      const ground = tinted(palette[base], palette['--accent'], tint / 100);
      for (const fg of texts) {
        const value = contrast(palette[fg], ground);
        if (value < 4.5) fail(`${label} theme: ${fg} on ${what} over ${base} is ${value.toFixed(2)}:1, WCAG needs 4.5:1`);
      }
    }
  }
}

// A label and its value, or the columns of a band, run together as one word
// where only CSS keeps them apart: a screen reader and a copy read
// "ReadsRadarr". A space in the markup separates them for both.
let bandsRead = 0;
for (const [file, html] of Object.entries(pages)) {
  for (const [block] of html.matchAll(/<p class="(?:flow-k|objection objection-head)">[\s\S]*?<\/p>/g)) {
    bandsRead++;
    if (/<\/(?:i|span)>(?:<span|[^\s<])/.test(block)) fail(`${file}: a label runs into what follows it: ${block.slice(0, 90)}`);
  }
}
if (bandsRead < 8) fail(`read ${bandsRead} label band(s), so the run-together check is reading almost nothing`);

// The page opens dark, and the browser's bar follows the page, never the
// system: every page stamps `data-theme="dark"`, carries one `theme-color`
// reading the dark ground, and `site.js` writes the light ground once a
// visitor picks it. A pair switched by `prefers-color-scheme` draws a light
// bar over a dark page whenever the system is light.
for (const [file, html] of Object.entries(pages)) {
  if (!/<html[^>]* data-theme="dark"/.test(html)) fail(`${file}: <html> does not open in the dark theme`);
  const colours = [...html.matchAll(/<meta name="theme-color"([^>]*)>/g)].map((m) => m[1]);
  if (colours.length !== 1 || !colours[0].includes(`content="${explicit['--bg']}"`) || colours[0].includes('media=')) {
    fail(`${file}: theme-color must be one tag reading the dark ground ${explicit['--bg']}, found ${colours.length}`);
  }
}
for (const ground of [light['--bg'], explicit['--bg']]) {
  if (!siteScript.includes(`'${ground}'`)) fail(`site.js never writes the ground ${ground} into theme-color`);
}

// `--focus` is the one ring colour measured above. A ring drawn in another
// colour is one nothing measures, and the accent on the light ground is far
// below 3:1.
const unmeasuredRings = [...css.matchAll(/outline(?:-color)?:\s*([^;]+);/g)]
  .map((m) => m[1].trim())
  .filter((value) => !/^(none|0)$/.test(value) && !value.includes('var(--focus)'));
if (unmeasuredRings.length) {
  fail(`site.css draws a ring in a colour other than --focus: ${unmeasuredRings.join(', ')}`);
}

// ------------------------------------------------- immutable means fingerprinted
// `immutable` for a year is a promise that the bytes at this URL never change.
// Astro fingerprints what it builds, so `/_astro/*` can keep it. Everything
// under `/assets/` is hand-placed and keeps its name, and a re-captured
// screenshot replaces its file under the same name: cached as immutable, the
// corrected file never reaches a returning visitor.
//
// The rule is the *name*, not the directory: every file under `/assets/`
// without a fingerprint needs a bounded rule of its own in `_headers`, so a
// file added there fails the build until it is given one.
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
  if (!immutable.size) fail('no `immutable` rule in _headers: the parser read nothing');

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
  if (served.length < 5) fail(`only ${served.length} file(s) found under /assets: the walk is wrong`);

  for (const file of served) {
    const url = `/${file}`;
    if (fingerprinted.test(url)) continue;
    const covered = [...bounded].some(
      (pattern) => pattern === url || (pattern.endsWith('*') && url.startsWith(pattern.slice(0, -1))),
    );
    if (!covered) {
      fail(
        `${url} carries no fingerprint and no bounded Cache-Control rule in _headers: ` +
          'every unfingerprinted file under /assets needs one, or a replacement in place stays unseen',
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
