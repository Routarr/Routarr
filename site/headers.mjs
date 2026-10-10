/**
 * The Cloudflare `_headers` format, read the way the Worker reads it: a path
 * line, then its indented headers. `serve.mjs` applies it to the preview, and
 * `live.mjs` holds production to it.
 */

/**
 * @param {string} source the text of `_headers`
 * @returns {{ pattern: string, headers: [string, string][] }[]}
 */
export function parseHeaders(source) {
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

function matches(pattern, path) {
  if (pattern.endsWith('/*')) return path.startsWith(pattern.slice(0, -1));
  return pattern === path;
}

/**
 * The headers every rule matching `path` sets, a later rule overriding an
 * earlier one, keyed by name as written.
 *
 * @param {ReturnType<typeof parseHeaders>} rules
 * @param {string} path
 * @returns {Record<string, string>}
 */
export function headersFor(rules, path) {
  const headers = {};
  for (const rule of rules) {
    if (matches(rule.pattern, path)) {
      for (const [name, value] of rule.headers) headers[name] = value;
    }
  }
  return headers;
}
