#!/usr/bin/env node
/**
 * Static server for the showcase site, for preview and verification.
 *
 * It parses `_headers` and applies it, which is the whole point: a
 * Content-Security-Policy that is only checked by reading it is not checked.
 * The Worker applies the same file in production.
 *
 *   node site/serve.mjs [port]
 */
import { createServer } from 'node:http';
import { readFile, stat } from 'node:fs/promises';
import { existsSync, readFileSync } from 'node:fs';
import { extname, join, normalize } from 'node:path';
import { fileURLToPath } from 'node:url';

import { headersFor, parseHeaders } from './headers.mjs';

const HERE = fileURLToPath(new URL('.', import.meta.url));

/**
 * Serves `dist/`, which is what Cloudflare serves, and parses the real
 * `_headers` on the way out, because previewing with any other static server
 * hides a CSP that blocks the stylesheet. That is the whole reason this file
 * exists rather than `astro preview`, which applies no headers at all.
 */
const ROOT = join(HERE, 'dist');
const PORT = Number(process.argv[2] ?? process.env.PORT ?? 8788);

/** The types production sends, with no charset: every page declares its own. */
const TYPES = {
  '.html': 'text/html',
  '.css': 'text/css',
  '.js': 'text/javascript',
  '.svg': 'image/svg+xml',
  '.png': 'image/png',
  '.ico': 'image/vnd.microsoft.icon',
  '.xml': 'application/xml',
  '.txt': 'text/plain',
  '.json': 'application/json',
};

/** What production answers for a file `_headers` gives no cache rule. */
const DEFAULT_CACHE = 'public, max-age=0, must-revalidate';

const RULES = parseHeaders(readFileSync(join(ROOT, '_headers'), 'utf-8'));

/**
 * Where a path resolves under `dist`, or the directory it names.
 *
 * `/fr` is a directory with an `index.html`, and Cloudflare answers it with a
 * redirect to `/fr/`. A preview that answered 404 would call broken a link one
 * character short that production serves.
 */
async function resolve(path) {
  const candidates = path.endsWith('/') ? [join(path, 'index.html')] : [path, `${path}.html`];
  for (const candidate of candidates) {
    const file = join(ROOT, normalize(candidate).replace(/^(\.\.[/\\])+/, ''));
    if (!file.startsWith(ROOT)) continue;
    try {
      if ((await stat(file)).isFile()) return file;
    } catch {
      /* try the next candidate */
    }
  }
  if (!path.endsWith('/')) {
    const dir = join(ROOT, normalize(path).replace(/^(\.\.[/\\])+/, ''));
    try {
      if (dir.startsWith(ROOT) && (await stat(join(dir, 'index.html'))).isFile()) {
        return { redirect: `${path}/` };
      }
    } catch {
      /* not a directory with an index */
    }
  }
  return null;
}

createServer(async (request, response) => {
  // A percent sequence that decodes to nothing is a 400, not an uncaught
  // exception that takes the preview down.
  let path;
  try {
    path = decodeURIComponent(new URL(request.url, 'http://localhost').pathname);
  } catch {
    response.writeHead(400, { 'Content-Type': 'text/plain; charset=utf-8' });
    response.end('malformed path');
    return;
  }
  // Production drops `.html` and `index.html` from an address with a 307.
  const pretty = path.replace(/\/index\.html$/, '/').replace(/\.html$/, '');
  if (pretty !== path) {
    response.writeHead(307, { Location: pretty });
    response.end();
    return;
  }
  const resolved = await resolve(path);
  // 307, as Cloudflare answers a directory named without its slash.
  if (resolved && typeof resolved === 'object') {
    response.writeHead(307, { Location: resolved.redirect });
    response.end();
    return;
  }
  const file = resolved;

  const headers = { 'Cache-Control': DEFAULT_CACHE, ...headersFor(RULES, path) };

  if (!file) {
    // The `404.html` nearest the missing path, as Cloudflare's `404-page`
    // handling serves it: a miss under `/fr/` reads French.
    const folders = path.split('/').slice(1, -1).filter((folder) => folder && folder !== '.' && folder !== '..');
    let notFound = join(ROOT, '404.html');
    for (let depth = folders.length; depth > 0; depth--) {
      const candidate = join(ROOT, ...folders.slice(0, depth), '404.html');
      if (existsSync(candidate)) {
        notFound = candidate;
        break;
      }
    }
    const body = await readFile(notFound);
    response.writeHead(404, { ...headers, 'Content-Type': TYPES['.html'] });
    return response.end(body);
  }

  const body = await readFile(file);
  response.writeHead(200, {
    ...headers,
    'Content-Type': TYPES[extname(file)] ?? 'application/octet-stream',
    'Content-Length': body.length,
  });
  response.end(body);
}).listen(PORT, '127.0.0.1', () => {
  console.log(`site on http://127.0.0.1:${PORT}`);
});
