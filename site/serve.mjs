#!/usr/bin/env node
/**
 * Static server for the showcase site — preview and verification.
 *
 * It parses `_headers` and applies it, which is the whole point: a
 * Content-Security-Policy that is only checked by reading it is not checked.
 * The Worker applies the same file in production.
 *
 *   node site/serve.mjs [port]
 */
import { createServer } from 'node:http';
import { readFile, stat } from 'node:fs/promises';
import { readFileSync } from 'node:fs';
import { extname, join, normalize } from 'node:path';
import { fileURLToPath } from 'node:url';

const HERE = fileURLToPath(new URL('.', import.meta.url));

/**
 * Serves `dist/`, which is what Cloudflare serves — and parses the real
 * `_headers` on the way out, because previewing with any other static server
 * hides a CSP that blocks the stylesheet. That is the whole reason this file
 * exists rather than `astro preview`, which applies no headers at all.
 */
const ROOT = join(HERE, 'dist');
const PORT = Number(process.argv[2] ?? process.env.PORT ?? 8788);

const TYPES = {
  '.html': 'text/html; charset=utf-8',
  '.css': 'text/css; charset=utf-8',
  '.js': 'text/javascript; charset=utf-8',
  '.svg': 'image/svg+xml',
  '.png': 'image/png',
  '.webp': 'image/webp',
  '.avif': 'image/avif',
  '.xml': 'application/xml; charset=utf-8',
  '.txt': 'text/plain; charset=utf-8',
  '.json': 'application/json; charset=utf-8',
};

/** Parse the Cloudflare `_headers` format: a path line, then indented headers. */
function parseHeaders(source) {
  const rules = [];
  let current = null;
  for (const raw of source.split('\n')) {
    const line = raw.replace(/\s+$/, '');
    if (!line || line.trimStart().startsWith('#')) continue;
    if (!/^\s/.test(line)) {
      current = { pattern: line.trim(), headers: [] };
      rules.push(current);
      continue;
    }
    const at = line.indexOf(':');
    if (current && at > 0) {
      current.headers.push([line.slice(0, at).trim(), line.slice(at + 1).trim()]);
    }
  }
  return rules;
}

const RULES = parseHeaders(readFileSync(join(ROOT, '_headers'), 'utf-8'));

function matches(pattern, path) {
  if (pattern.endsWith('/*')) return path.startsWith(pattern.slice(0, -1));
  return pattern === path;
}

/**
 * Where a path resolves under `dist`, or the directory it names.
 *
 * `/fr` is a directory with an `index.html`; Cloudflare answers it with a
 * redirect to `/fr/`, and a preview that answered 404 hid nothing real but
 * made a link one character short look broken.
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
  const resolved = await resolve(path);
  if (resolved && typeof resolved === 'object') {
    response.writeHead(301, { Location: resolved.redirect });
    response.end();
    return;
  }
  const file = resolved;

  const headers = {};
  for (const rule of RULES) {
    if (matches(rule.pattern, path)) {
      for (const [name, value] of rule.headers) headers[name] = value;
    }
  }

  if (!file) {
    const body = await readFile(join(ROOT, '404.html'));
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
