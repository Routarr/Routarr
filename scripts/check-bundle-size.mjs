#!/usr/bin/env node
/**
 * A ceiling on the script a first visit downloads before anything is shown, and another on the
 * stylesheet `dist/index.html` links, which every visit downloads before its first paint. Each
 * screen is its own chunk, and one static import can pull another into the first load. The files
 * are those the page names (the entry and every module it preloads), read from it rather than
 * matched by name, and measured raw.
 *
 *   node scripts/check-bundle-size.mjs      # after `npm run build` in frontend/
 */
import { readFileSync, statSync } from 'node:fs';
import { fileURLToPath } from 'node:url';

const DIST = fileURLToPath(new URL('../frontend/dist/', import.meta.url));
const CEILING = 124_000;
const STYLE_CEILING = 55_000;

let page;
try {
  page = readFileSync(`${DIST}index.html`, 'utf-8');
} catch {
  console.error(`check-bundle-size: ${DIST}index.html does not exist: run \`npm run build\` in frontend/ first`);
  process.exit(1);
}

const named = (pattern) => [...page.matchAll(pattern)].map((match) => match[1].replace(/^\.?\//, ''));
const files = [
  ...named(/<script type="module"[^>]*\ssrc="([^"]+\.js)"/g),
  ...named(/<link rel="modulepreload"[^>]*\shref="([^"]+\.js)"/g),
];
// A page whose entry the pattern no longer finds would pass measuring nothing.
if (!files.some((file) => /^assets\/index-[\w-]+\.js$/.test(file))) {
  console.error('check-bundle-size: dist/index.html names no entry script the check recognises');
  process.exit(1);
}

// The same holds for the stylesheet: a link the pattern misses would cap nothing.
const styles = named(/<link rel="stylesheet"[^>]*\shref="([^"]+\.css)"/g);
if (styles.length === 0) {
  console.error('check-bundle-size: dist/index.html links no stylesheet the check recognises');
  process.exit(1);
}

/** Lists each file, and answers whether together they stay within `ceiling`. */
function within(what, names, ceiling) {
  let total = 0;
  for (const file of names) {
    const size = statSync(`${DIST}${file}`).size;
    total += size;
    console.log(`  ${file}: ${size} bytes`);
  }
  const verdict = total > ceiling ? 'over' : 'within';
  console.log(`${what}: ${names.length} files, ${total} bytes, ${verdict} the ${ceiling}-byte ceiling`);
  if (total > ceiling) console.error(`check-bundle-size: ${what} is ${total} bytes, over ${ceiling}`);
  return total <= ceiling;
}

const scripts = within('the first load', files, CEILING);
const stylesheet = within('the stylesheet', styles, STYLE_CEILING);
if (!scripts || !stylesheet) process.exit(1);
