import { describe, it, expect } from 'vitest';
import fs from 'node:fs';
import path from 'node:path';

import { ENGLISH } from './render';

const SRC = path.resolve(__dirname, '..');

/** The `{name}` placeholders of a template, as `t()` substitutes them. */
const placeholders = (template: string) =>
  new Set([...template.matchAll(/\{(\w+)\}/g)].map((match) => match[1] ?? ''));

/**
 * The application's own sources that call `t()`: every `.svelte` and `.ts`
 * file but the tests and their helpers, and only where `t` is the one imported
 * from the dictionary, so a comment naming `t()` in another file is not read
 * as a call.
 */
function sources(): string[] {
  return fs
    .readdirSync(SRC, { recursive: true, encoding: 'utf-8' })
    .map((file) => file.split(path.sep).join('/'))
    .filter((file) => /\.(svelte|ts)$/.test(file) && !file.endsWith('.test.ts'))
    .filter((file) => !file.startsWith('test/'))
    .filter((file) => /import \{[^}]*\bt\b[^}]*\} from '[^']*i18n\.svelte'/.test(read(file)));
}

function read(file: string): string {
  return fs.readFileSync(path.join(SRC, file), 'utf-8');
}

/**
 * The source with its comments blanked, line numbers kept. A `//` counts only
 * after a space or at the start of a line, since one after a colon is a URL.
 */
function code(source: string): string {
  const blank = (match: string) => match.replace(/[^\n]/g, ' ');
  return source
    .replace(/<!--[\s\S]*?-->/g, blank)
    .replace(/\/\*[\s\S]*?\*\//g, blank)
    .replace(
      /(^|\s)\/\/.*$/gm,
      (whole, before: string) => before + blank(whole.slice(before.length)),
    );
}

/** The index just past the string literal that opens at `start`, template literals included. */
function skipString(source: string, start: number): number {
  const quote = source[start];
  let cursor = start + 1;
  while (cursor < source.length) {
    const char = source[cursor];
    if (char === '\\') cursor += 2;
    else if (char === quote) return cursor + 1;
    else if (quote === '`' && char === '$' && source[cursor + 1] === '{') {
      cursor = split(source, cursor + 1).end + 1;
    } else cursor += 1;
  }
  return cursor;
}

/**
 * The top-level, comma-separated parts of the bracketed list that opens at
 * `open`, and the index of its closing bracket. Brackets and strings are
 * skipped whole, so a comma inside either does not split.
 */
function split(source: string, open: number): { parts: string[]; end: number } {
  const parts: string[] = [];
  let depth = 0;
  let start = open + 1;
  let cursor = open + 1;
  while (cursor < source.length) {
    const char = source[cursor]!;
    if (char === "'" || char === '"' || char === '`') {
      cursor = skipString(source, cursor);
      continue;
    }
    if ('([{'.includes(char)) depth += 1;
    else if (')]}'.includes(char)) {
      if (depth === 0) break;
      depth -= 1;
    } else if (char === ',' && depth === 0) {
      parts.push(source.slice(start, cursor));
      start = cursor + 1;
    }
    cursor += 1;
  }
  parts.push(source.slice(start, cursor));
  return { parts: parts.map((part) => part.trim()).filter(Boolean), end: cursor };
}

interface Call {
  where: string;
  /** The keys the first argument can name, none when it is built at run time. */
  keys: string[];
  /** The parameter names of an object literal, `null` when they are built at run time. */
  params: string[] | null;
  /** Whether the call passes a second argument at all. */
  passes: boolean;
}

/** The literal key, or the two keys of a ternary between literals. */
function keysOf(argument: string): string[] {
  const literal = /^(['"`])(\w+)\1$/.exec(argument);
  if (literal) return [literal[2]!];
  const ternary = /^[\s\S]+?\?\s*(['"])(\w+)\1\s*:\s*(['"])(\w+)\3$/.exec(argument);
  return ternary ? [ternary[2]!, ternary[4]!] : [];
}

/** The property names of an object literal, or `null` where one is built at run time. */
function namesOf(argument: string): string[] | null {
  if (!argument.startsWith('{')) return null;
  const names: string[] = [];
  for (const property of split(argument, 0).parts) {
    const name = /^(['"]?)(\w+)\1\s*(?::|$)/.exec(property)?.[2];
    if (name === undefined) return null;
    names.push(name);
  }
  return names;
}

function calls(): Call[] {
  return sources().flatMap((file) => {
    const source = code(read(file));
    return [...source.matchAll(/(?<![\w.$])t\(/g)].map((match) => {
      const open = (match.index ?? 0) + match[0].length - 1;
      const [key = '', params] = split(source, open).parts;
      return {
        where: `${file}:${source.slice(0, open).split('\n').length}`,
        keys: keysOf(key),
        params: params === undefined ? [] : namesOf(params),
        passes: params !== undefined,
      };
    });
  });
}

/**
 * The calls whose parameters are built at run time, by file and key, each with
 * the reason. The source holds no names to compare, so the component's own
 * test is what sees a placeholder left unfilled.
 */
const BUILT_AT_RUN_TIME: Record<string, string> = {
  'components/GuideStepBanner.svelte GuideOptionalDoneNext': 'the next step, named once for three',
  'components/GuideStepBanner.svelte GuideOptionalNext': 'the next step, named once for three',
  'components/GuideStepBanner.svelte GuideStepDoneNext': 'the next step, named once for three',
};

/**
 * A placeholder a call leaves out reaches the reader as `{name}`, and a name
 * the English does not have is a value nobody sees, most often a placeholder
 * renamed on one side only. A call with no second argument passes no name, so
 * a key whose English has a placeholder needs one. A ternary between two keys
 * hands both the same object: each needs all of its own placeholders, and the
 * object names nothing neither of them has.
 */
describe('every call to t() passes the placeholders its English names', () => {
  const found = calls();
  const checked = found.filter((call) => call.keys.length > 0 && call.params !== null);

  it('reads the calls of every screen', () => {
    expect(sources().length).toBeGreaterThan(50);
    expect(checked.length).toBeGreaterThan(500);
    expect(checked.filter((call) => call.passes).length).toBeGreaterThan(100);
    expect(checked.filter((call) => call.keys.length === 2).length).toBeGreaterThan(20);
  });

  it('omits no placeholder and passes no name the English does not have', () => {
    const wrong = checked.flatMap(({ where, keys, params }) => {
      const absent = keys.filter((key) => ENGLISH[key] === undefined);
      if (absent.length > 0) return absent.map((key) => `${where} ${key}: not in en.json`);
      const wanted = keys.map((key) => ({ key, names: placeholders(ENGLISH[key]!) }));
      return [
        ...wanted.flatMap(({ key, names }) =>
          [...names]
            .filter((name) => !params!.includes(name))
            .map((name) => `${where} ${key}: omits {${name}}`),
        ),
        ...params!
          .filter((name) => !wanted.some(({ names }) => names.has(name)))
          .map((name) => `${where} ${keys.join(' or ')}: passes ${name}, which the English lacks`),
      ];
    });
    expect(wrong).toEqual([]);
  });

  it('lists every call whose parameters are built at run time, and no other', () => {
    const built = found
      .filter((call) => call.keys.length > 0 && call.params === null)
      .flatMap((call) => call.keys.map((key) => `${call.where.split(':')[0]} ${key}`));
    expect([...new Set(built)].sort()).toEqual(Object.keys(BUILT_AT_RUN_TIME).sort());
  });
});
