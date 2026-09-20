// Regenerate the raster icons from public/assets/favicon.svg.
//
// An SVG favicon alone is not enough. Every current browser prefers it, but
// older Safari ignores `type="image/svg+xml"` outright, and a fair number of
// crawlers and link unfurlers request `/favicon.ico` at the origin root without
// ever reading the document's <link> tags.
//
// The SVG stays the source of truth: this script renders it rather than asking
// anyone to keep three drawings in step by hand. `node site/check.mjs` fails if
// an output is missing or if its real size stops matching the `sizes` attribute
// that announces it.
//
// Chromium does the rasterising because it is already installed for the
// screenshots and the CSP verification — no image library enters the
// dependency tree for three files that change once a year.

import { createRequire } from 'node:module';
import { readFileSync, writeFileSync } from 'node:fs';
import { fileURLToPath } from 'node:url';
import { join } from 'node:path';

const ROOT = fileURLToPath(new URL('.', import.meta.url));
// Same resolution as verify.mjs: the site has no package.json of its own, and
// is not going to grow one for a browser the frontend already depends on.
const require = createRequire(`${process.env.FRONTEND_DIR ?? `${ROOT}../frontend`}/package.json`);
const { chromium } = require('@playwright/test');
const svg = readFileSync(join(ROOT, 'public/assets/favicon.svg'), 'utf-8');
const dataUri = `data:image/svg+xml;base64,${Buffer.from(svg).toString('base64')}`;

// The dark surface from site.css. Apple composites its touch icon over an
// opaque square whatever the image says, so the padding is drawn here rather
// than left to guesswork.
const APPLE_BACKGROUND = '#131519';

const browser = await chromium.launch();
const page = await browser.newPage();

async function render({ size, background, scale }) {
  await page.setViewportSize({ width: size, height: size });
  await page.setContent(
    `<style>
       html,body{margin:0;padding:0;width:${size}px;height:${size}px}
       body{background:${background ?? 'transparent'};display:grid;place-items:center}
       img{width:${Math.round(size * scale)}px;height:${Math.round(size * scale)}px}
     </style>
     <img src="${dataUri}" alt="">`,
  );
  return page.screenshot({ omitBackground: !background });
}

// A 32×32 PNG: the fallback the document offers alongside the SVG.
writeFileSync(join(ROOT, 'public/assets/favicon-32.png'), await render({ size: 32, scale: 1 }));

// 180×180 is what iOS asks for, on an opaque tile with breathing room.
writeFileSync(
  join(ROOT, 'public/assets/apple-touch-icon.png'),
  await render({ size: 180, scale: 0.62, background: APPLE_BACKGROUND }),
);

// ---------------------------------------------------------------- favicon.ico
//
// Read back as raw pixels rather than by decoding a PNG: the canvas already
// holds exactly what we drew. The payload is an uncompressed 32-bit BMP, not an
// embedded PNG — a PNG inside an ICO is only understood from Windows Vista on,
// and this file exists precisely for the clients that understand the least.
const pixels = await page.evaluate(async (src) => {
  const image = new Image();
  image.src = src;
  await image.decode();
  const canvas = document.createElement('canvas');
  canvas.width = 32;
  canvas.height = 32;
  const context = canvas.getContext('2d');
  context.drawImage(image, 0, 0, 32, 32);
  return [...context.getImageData(0, 0, 32, 32).data];
}, dataUri);

await browser.close();

function ico(rgba, size) {
  // BGRA, bottom-up, which is how a BMP stores its rows.
  const xor = Buffer.alloc(size * size * 4);
  for (let y = 0; y < size; y++) {
    for (let x = 0; x < size; x++) {
      const from = (y * size + x) * 4;
      const to = ((size - 1 - y) * size + x) * 4;
      xor[to] = rgba[from + 2];
      xor[to + 1] = rgba[from + 1];
      xor[to + 2] = rgba[from];
      xor[to + 3] = rgba[from + 3];
    }
  }
  // The 1-bit AND mask is mandatory even when the alpha channel already carries
  // transparency. All zeroes means "take the alpha channel's word for it".
  const mask = Buffer.alloc((size / 8) * size);

  const header = Buffer.alloc(40);
  header.writeUInt32LE(40, 0);
  header.writeInt32LE(size, 4);
  header.writeInt32LE(size * 2, 8); // XOR and AND stacked, as BMP demands
  header.writeUInt16LE(1, 12);
  header.writeUInt16LE(32, 14);
  header.writeUInt32LE(xor.length + mask.length, 20);

  const image = Buffer.concat([header, xor, mask]);
  const directory = Buffer.alloc(22);
  directory.writeUInt16LE(0, 0);
  directory.writeUInt16LE(1, 2); // 1 = icon
  directory.writeUInt16LE(1, 4); // one image
  directory.writeUInt8(size, 6);
  directory.writeUInt8(size, 7);
  directory.writeUInt16LE(1, 10);
  directory.writeUInt16LE(32, 12);
  directory.writeUInt32LE(image.length, 14);
  directory.writeUInt32LE(directory.length, 18);

  return Buffer.concat([directory, image]);
}

writeFileSync(join(ROOT, 'public/favicon.ico'), ico(pixels, 32));

console.log('wrote public/assets/favicon-32.png, public/assets/apple-touch-icon.png, public/favicon.ico');
