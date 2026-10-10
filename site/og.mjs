// Render the link preview card, one per language, from og.html.
//
// Each `data-key` of the template takes its value from that language's
// catalogue, so the card a French link unfurls reads French, and a headline
// edited in the hero reaches its card at the next run. The card is a fixed
// 1200x630 frame that clips what it cannot hold, so a translation too long
// for it fails here rather than shipping cut off.
//
// Chromium does the rendering for the reason `icons.mjs` gives: it is already
// installed, and no image library enters the dependency tree.

import { readFileSync, writeFileSync } from 'node:fs';
import { join } from 'node:path';
import { fileURLToPath, pathToFileURL } from 'node:url';

import { chromium } from './playwright.mjs';

const ROOT = fileURLToPath(new URL('.', import.meta.url));
const { LANGUAGES } = await import('./src/i18n/languages.ts');
/** The padding of og.html's body: text past it touches the card's edge. */
const MARGIN = { x: 96, y: 74 };

const browser = await chromium.launch();
const page = await browser.newPage({ viewport: { width: 1200, height: 630 }, deviceScaleFactor: 1 });
const failures = [];

for (const { code } of LANGUAGES) {
  const catalogue = JSON.parse(readFileSync(join(ROOT, `src/i18n/${code}.json`), 'utf-8'));
  await page.goto(pathToFileURL(join(ROOT, 'og.html')).href);
  const { missing, outside } = await page.evaluate(
    ({ code, catalogue, margin }) => {
      document.documentElement.lang = code;
      const missing = [];
      for (const element of document.querySelectorAll('[data-key]')) {
        const value = catalogue[element.dataset.key];
        if (value) element.innerHTML = value;
        else missing.push(element.dataset.key);
      }
      const outside = [...document.querySelectorAll('h1, p, li')]
        .filter((element) => {
          const box = element.getBoundingClientRect();
          return box.right > innerWidth - margin.x + 1 || box.bottom > innerHeight - margin.y + 1
            || element.scrollWidth > element.clientWidth + 1;
        })
        .map((element) => element.textContent.trim());
      return { missing, outside };
    },
    { code, catalogue, margin: MARGIN },
  );
  for (const key of missing) failures.push(`${code}: the catalogue has no ${key}`);
  for (const text of outside) failures.push(`${code}: "${text}" does not fit the card`);
  writeFileSync(join(ROOT, `public/assets/og-${code}.png`), await page.screenshot());
}

await browser.close();

if (failures.length) {
  console.error(failures.join('\n'));
  process.exit(1);
}
console.log(`wrote ${LANGUAGES.map(({ code }) => `public/assets/og-${code}.png`).join(', ')}`);
