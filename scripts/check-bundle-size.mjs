#!/usr/bin/env node
/**
 * A ceiling on the script a first visit downloads before anything is shown.
 *
 * Each screen is its own chunk, so opening the dashboard does not download the
 * rule builder with it. That property drifts one import at a time, and only a
 * ceiling notices. What the first load fetches is what `dist/index.html`
 * names: the entry, and every module Vite preloads because the entry imports
 * it statically, the shared runtime among them. A module the shell imports
 * statically is fetched there whatever chunk it is built into, so the files
 * are read from the page, not matched by name. Measured raw, the way the
 * fingerprinted files sit in `dist/assets`.
 *
 *   node scripts/check-bundle-size.mjs      # after `npm run build` in frontend/
 */
import { readFileSync, statSync } from 'node:fs';
import { fileURLToPath } from 'node:url';

const DIST = fileURLToPath(new URL('../frontend/dist/', import.meta.url));
const CEILING = 124_000;

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

let total = 0;
for (const file of files) {
  const size = statSync(`${DIST}${file}`).size;
  total += size;
  console.log(`  ${file}: ${size} bytes`);
}
const verdict = total > CEILING ? 'over' : 'within';
console.log(`the first load: ${files.length} files, ${total} bytes, ${verdict} the ${CEILING}-byte ceiling`);
if (total > CEILING) {
  console.error(`check-bundle-size: the first load is ${total} bytes; the ceiling is ${CEILING}`);
  process.exit(1);
}
