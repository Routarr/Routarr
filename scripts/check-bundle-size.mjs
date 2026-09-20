#!/usr/bin/env node
/**
 * Ceilings on the two bundles every visit downloads.
 *
 * Each screen is its own chunk, so opening the dashboard does not download the
 * rule builder with it — a property nothing measured, and the kind that drifts
 * one import at a time. The entry carries the shell and the router; the shared
 * runtime carries Svelte and the icons. Both measured raw, the way the
 * fingerprinted files sit in `dist/assets`.
 *
 *   node scripts/check-bundle-size.mjs      # after `npm run build` in frontend/
 */
import { readdirSync, statSync } from 'node:fs';
import { fileURLToPath } from 'node:url';

const ASSETS = fileURLToPath(new URL('../frontend/dist/assets/', import.meta.url));

const CEILINGS = [
  { name: 'the entry bundle', pattern: /^index-[\w-]+\.js$/, ceiling: 25_000 },
  { name: 'the shared runtime', pattern: /^vendor-[\w-]+\.js$/, ceiling: 75_000 },
];

let files;
try {
  files = readdirSync(ASSETS);
} catch {
  console.error(`check-bundle-size: ${ASSETS} does not exist — run \`npm run build\` in frontend/ first`);
  process.exit(1);
}

const failures = [];
for (const { name, pattern, ceiling } of CEILINGS) {
  const matches = files.filter((file) => pattern.test(file));
  // A pattern that matches nothing would pass without measuring; that is not a pass.
  if (matches.length !== 1) {
    failures.push(`${name}: expected one file matching ${pattern}, found ${matches.length}`);
    continue;
  }
  const size = statSync(`${ASSETS}${matches[0]}`).size;
  const verdict = size > ceiling ? 'over' : 'within';
  console.log(`${name}: ${matches[0]} is ${size} bytes, ${verdict} the ${ceiling}-byte ceiling`);
  if (size > ceiling) failures.push(`${name} (${matches[0]}) is ${size} bytes; the ceiling is ${ceiling}`);
}

if (failures.length) {
  for (const failure of failures) console.error(`check-bundle-size: ${failure}`);
  process.exit(1);
}
