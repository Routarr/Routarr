/**
 * A Markdown copy of every page, for language models: `llms.txt` links them,
 * at each page's address with `index.html.md` appended, as llmstxt.org
 * proposes. Written from the built `<main>` once the build is done, so a copy
 * says exactly what its page says, in its language.
 *
 * The HTML is Astro's own output for this site, so a small reader of tags is
 * enough and no parser enters the dependency tree. What a reader of the page
 * never meets is left out: a decoration hidden from assistive technology, an
 * icon, a button. Text kept for a screen reader stays, since it carries words
 * the eye takes from the layout.
 */
import { readFileSync, writeFileSync } from 'node:fs';
import { fileURLToPath } from 'node:url';

const VOID = new Set(['area', 'base', 'br', 'col', 'embed', 'hr', 'img', 'input', 'link', 'meta', 'source', 'track', 'wbr']);
const SKIPPED = new Set(['button', 'script', 'style', 'svg', 'template']);
const CONTAINERS = new Set(['article', 'aside', 'details', 'div', 'figure', 'footer', 'header', 'main', 'nav', 'section']);
const BLOCKS = new Set([...CONTAINERS, 'dl', 'h1', 'h2', 'h3', 'h4', 'h5', 'h6', 'ol', 'p', 'pre', 'summary', 'table', 'ul']);

const ENTITIES = { amp: '&', lt: '<', gt: '>', quot: '"', apos: "'", nbsp: ' ' };
const decode = (text) =>
  text.replace(/&(#x[0-9a-f]+|#\d+|[a-z]+);/gi, (entity, name) => {
    if (name[0] === '#') return String.fromCodePoint(name[1] === 'x' ? parseInt(name.slice(2), 16) : Number(name.slice(1)));
    return ENTITIES[name] ?? entity;
  });

/** The element tree of an HTML fragment. */
function parse(html) {
  const root = { tag: '#root', attrs: {}, children: [] };
  const stack = [root];
  for (const [, comment, closing, tag, attributes, text] of html.matchAll(
    /(<!--[\s\S]*?-->)|<(\/?)([a-zA-Z][\w-]*)((?:[^>"']|"[^"]*"|'[^']*')*)>|([^<]+)/g,
  )) {
    const parent = stack[stack.length - 1];
    if (comment) continue;
    if (text !== undefined) {
      parent.children.push(decode(text));
    } else if (closing) {
      const at = stack.findLastIndex((node) => node.tag === tag.toLowerCase());
      if (at > 0) stack.length = at;
    } else {
      const attrs = Object.fromEntries(
        [...attributes.matchAll(/([\w:-]+)(?:=(?:"([^"]*)"|'([^']*)'|([^\s>]+)))?/g)].map((m) => [
          m[1].toLowerCase(),
          decode(m[2] ?? m[3] ?? m[4] ?? ''),
        ]),
      );
      const node = { tag: tag.toLowerCase(), attrs, children: [] };
      parent.children.push(node);
      if (!VOID.has(node.tag) && !attributes.trim().endsWith('/')) stack.push(node);
    }
  }
  return root;
}

const silent = (node) =>
  SKIPPED.has(node.tag) || node.attrs['aria-hidden'] === 'true' || 'hidden' in node.attrs;
const textOf = (node) => (typeof node === 'string' ? node : node.children.map(textOf).join(''));

/**
 * Inline content as one line of Markdown. Two elements side by side with no
 * text between them are kept apart by a space: on the page a layout parts
 * them, and run together they read "plandry-run".
 */
function inline(nodes, base) {
  let previous = null;
  return nodes
    .map((node) => {
      if (typeof node === 'string') {
        previous = null;
        return node.replace(/[ \t\r\n\f]+/g, ' ');
      }
      const part = element(node, base);
      const apart = previous !== null && /\S$/.test(previous) && /^\S/.test(part);
      if (part) previous = part;
      return apart ? ` ${part}` : part;
    })
    .join('');
}

/** One inline element as Markdown. */
function element(node, base) {
  if (silent(node)) return '';
  const inner = () => inline(node.children, base);
  switch (node.tag) {
    case 'br':
      return '\n';
    case 'code':
    case 'kbd':
      return `\`${textOf(node).replace(/\s+/g, ' ').trim()}\``;
    case 'strong':
    case 'b': {
      const text = inner().trim();
      return text ? `**${text}**` : '';
    }
    case 'em':
    case 'i': {
      const text = inner().trim();
      return text ? `*${text}*` : '';
    }
    case 'a': {
      const text = inner().trim();
      return text ? `[${text}](${new URL(node.attrs.href ?? '', base).href})` : '';
    }
    default:
      return BLOCKS.has(node.tag) ? ` ${blocks([node], base).join(' ')} ` : inner();
  }
}

// A space before a comma is a layout's: two elements parted on the page.
const clean = (line) => line.replace(/ +/g, ' ').replace(/ ,/g, ',').replace(/ ?\n ?/g, '\n').trim();
const cell = (node, base) => clean(inline(node.children, base)).replace(/\|/g, '\\|').replace(/\n/g, ' ');

/** Block content as Markdown blocks, one string each. */
function blocks(nodes, base) {
  const out = [];
  let run = [];
  const flush = () => {
    const text = clean(inline(run, base));
    if (text) out.push(text);
    run = [];
  };
  for (const node of nodes) {
    if (typeof node === 'string' || !BLOCKS.has(node.tag)) {
      run.push(node);
      continue;
    }
    flush();
    if (silent(node)) continue;
    const level = /^h([1-6])$/.exec(node.tag)?.[1];
    if (level) {
      out.push(`${'#'.repeat(Number(level))} ${clean(inline(node.children, base))}`);
    } else if (node.tag === 'p') {
      const text = clean(inline(node.children, base));
      if (text) out.push(text);
    } else if (node.tag === 'summary') {
      const text = clean(inline(node.children, base));
      if (text) out.push(`**${text}**`);
    } else if (node.tag === 'pre') {
      out.push(`\`\`\`\n${textOf(node).replace(/\n+$/, '')}\n\`\`\``);
    } else if (node.tag === 'ul' || node.tag === 'ol') {
      const items = node.children.filter((child) => typeof child !== 'string' && child.tag === 'li' && !silent(child));
      out.push(items.map((item, at) => {
        const marker = node.tag === 'ol' ? `${at + 1}. ` : '- ';
        return marker + blocks(item.children, base).join('\n\n').replace(/\n/g, `\n${' '.repeat(marker.length)}`);
      }).join('\n'));
    } else if (node.tag === 'dl') {
      const lines = [];
      for (const child of node.children) {
        if (typeof child === 'string' || silent(child)) continue;
        const text = clean(inline(child.children, base)).replace(/\n/g, ' ');
        if (child.tag === 'dt') lines.push(`- **${text}**`);
        else if (child.tag === 'dd' && lines.length) lines[lines.length - 1] += `: ${text}`;
      }
      if (lines.length) out.push(lines.join('\n'));
    } else if (node.tag === 'table') {
      out.push(table(node, base));
    } else {
      out.push(...blocks(node.children, base));
    }
  }
  flush();
  return out;
}

function table(node, base) {
  const rows = [];
  let caption = '';
  const walk = (parent) => {
    for (const child of parent.children) {
      if (typeof child === 'string' || silent(child)) continue;
      if (child.tag === 'caption') caption = clean(inline(child.children, base));
      else if (child.tag === 'tr') {
        rows.push(child.children.filter((c) => typeof c !== 'string' && (c.tag === 'th' || c.tag === 'td')).map((c) => cell(c, base)));
      } else walk(child);
    }
  };
  walk(node);
  if (!rows.length) return caption;
  const [head, ...body] = rows;
  const lines = [`| ${head.join(' | ')} |`, `| ${head.map(() => '---').join(' | ')} |`, ...body.map((row) => `| ${row.join(' | ')} |`)];
  return [caption, lines.join('\n')].filter(Boolean).join('\n\n');
}

/** A page's `<main>` as Markdown, its links absolute against `address`. */
export function toMarkdown(html, address) {
  const main = html.match(/<main\b[^>]*>([\s\S]*)<\/main>/)?.[1] ?? '';
  return `${blocks(parse(main).children, address).join('\n\n').replace(/\n +\n/g, '\n\n')}\n`;
}

/** The integration: one copy beside each page the build wrote, not-found pages left out. */
export function markdownCopies() {
  let site = '';
  return {
    name: 'routarr:markdown-copies',
    hooks: {
      'astro:config:done': ({ config }) => {
        site = config.site;
      },
      'astro:build:done': ({ dir, pages }) => {
        for (const { pathname } of pages) {
          if (/(^|\/)404\/?$/.test(pathname)) continue;
          const file = fileURLToPath(new URL(`${pathname}index.html`, dir));
          const address = new URL(pathname, site).href;
          writeFileSync(`${file}.md`, toMarkdown(readFileSync(file, 'utf-8'), address));
        }
      },
    },
  };
}
