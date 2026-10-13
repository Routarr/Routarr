// Render the raster icons from public/assets/favicon.svg, the one drawing to edit.
//
// Older Safari ignores an SVG favicon, and many crawlers fetch `/favicon.ico` without reading
// the page's <link> tags. `node site/check.mjs` fails when an output is missing or its size
// no longer matches its `sizes` attribute. Chromium, already installed for the screenshots,
// does the rendering, so no image library is needed.

import { readFileSync, writeFileSync } from 'node:fs';
import { fileURLToPath } from 'node:url';
import { join } from 'node:path';

import { chromium } from './playwright.mjs';

const ROOT = fileURLToPath(new URL('.', import.meta.url));
const svg = readFileSync(join(ROOT, 'public/assets/favicon.svg'), 'utf-8');
const dataUri = `data:image/svg+xml;base64,${Buffer.from(svg).toString('base64')}`;

// The dark surface from site.css. iOS shows a touch icon on an opaque square whatever its
// alpha, so the padding is drawn here in a known colour.
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
// An uncompressed 32-bit BMP rather than an embedded PNG: a PNG inside an ICO needs
// Windows Vista or later, and this file is for the clients that understand the least.
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
  // A BMP stores its rows bottom-up, in BGRA.
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
  // The 1-bit AND mask is required even with an alpha channel. All zeroes defers to the alpha.
  const mask = Buffer.alloc((size / 8) * size);

  const header = Buffer.alloc(40);
  header.writeUInt32LE(40, 0);
  header.writeInt32LE(size, 4);
  header.writeInt32LE(size * 2, 8); // an ICO counts the XOR and AND masks stacked
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
