import { describe, it, expect } from 'vitest';
import { readFileSync } from 'node:fs';
import { join } from 'node:path';

/**
 * WCAG 2.1 AA contrast for the two palettes, measured rather than eyeballed.
 *
 * An accent at 4.06:1 looks perfectly fine in a screenshot, and the light theme
 * is where that margin is thinnest. Colours are chosen by a person; whether
 * they can be read is arithmetic.
 */
// Resolved from the project root rather than from `import.meta.url`: the test
// environment does not serve this file over a `file:` URL.
const CSS = readFileSync(join(process.cwd(), 'src/index.css'), 'utf8');

/** The custom properties declared in one block. */
function tokensOf(pattern: RegExp): Record<string, string> {
  const body = CSS.match(pattern)?.[1] ?? '';
  const tokens: Record<string, string> = {};
  for (const [, name, value] of body.matchAll(/(--[\w-]+)\s*:\s*([^;]+);/g)) {
    // Both groups are in the pattern, so a match always has them — but the
    // compiler only knows what the type says, and asserting with `!` would
    // trade a named failure for `undefined is not a function`.
    if (name === undefined || value === undefined) continue;
    tokens[name] = value.trim();
  }
  return tokens;
}

function channels(colour: string): [number, number, number, number] {
  const hex = colour.match(/^#([0-9a-f]{6})$/i)?.[1];
  if (hex) {
    const n = parseInt(hex, 16);
    return [(n >> 16) & 255, (n >> 8) & 255, n & 255, 1];
  }
  const rgba = colour.match(/rgba?\(([^)]+)\)/)?.[1];
  if (rgba) {
    const [r, g, b, a] = rgba.split(',').map((p) => parseFloat(p.trim()));
    // A malformed `rgb()` in the stylesheet should name itself rather than
    // become `NaN` and fail somewhere down the arithmetic.
    if (r === undefined || g === undefined || b === undefined) {
      throw new Error(`incomplete colour: ${colour}`);
    }
    return [r, g, b, a ?? 1];
  }
  throw new Error(`unsupported colour: ${colour}`);
}

/** Flatten a translucent colour onto what sits behind it. */
function over(foreground: string, background: string): string {
  const [r, g, b, a] = channels(foreground);
  const [br, bg, bb] = channels(background);
  const mix = (f: number, k: number) => Math.round(f * a + k * (1 - a));
  return `#${[mix(r, br), mix(g, bg), mix(b, bb)]
    .map((c) => c.toString(16).padStart(2, '0'))
    .join('')}`;
}

function luminance(colour: string): number {
  const [r, g, b] = channels(colour);
  const toLinear = (c: number) =>
    c / 255 <= 0.03928 ? c / 255 / 12.92 : ((c / 255 + 0.055) / 1.055) ** 2.4;
  return 0.2126 * toLinear(r) + 0.7152 * toLinear(g) + 0.0722 * toLinear(b);
}

function contrast(a: string, b: string): number {
  const la = luminance(a);
  const lb = luminance(b);
  const [hi, lo] = la >= lb ? [la, lb] : [lb, la];
  return (hi + 0.05) / (lo + 0.05);
}

const dark = tokensOf(/^:root\s*\{([\s\S]*?)\n\}/m);
// The light block only redefines what changes, so the rest is inherited — the
// merge is what the browser actually resolves.
const light = { ...dark, ...tokensOf(/^:root\[data-theme='light'\]\s*\{([\s\S]*?)\n\}/m) };

/**
 * The same palette a second time, for the setting that stamps no attribute.
 *
 * `auto` is a mode, not an absence: it resolves through
 * `prefers-color-scheme`, so the light values have to be written again under a
 * selector no `[data-theme]` block can share. Nothing but this makes the two
 * agree, and a token declared in one and forgotten in the other renders its
 * dark value on a white ground for every reader who never opened the setting.
 */
const AUTO = /:root:not\(\[data-theme='dark'\]\)\s*\{([\s\S]*?)\n\s{2}\}/m;
const auto = { ...dark, ...tokensOf(AUTO) };

/** `[foreground, background]`, both resolved through the palette. */
const PAIRS: [string, string][] = [
  ['--text-primary', '--bg-base'],
  ['--text-primary', '--bg-card'],
  ['--text-secondary', '--bg-card'],
  ['--text-muted', '--bg-card'],
  ['--text-muted', '--bg-base'],
  // The accent as *text*, which is why it is a token of its own: the fill
  // colour is unreadable in that role on a light background.
  ['--accent-strong', '--bg-card'],
  ['--accent-strong', '--bg-base'],
  // A navigation entry at rest, on the sidebar's own surface. Brighter than
  // `--text-secondary` so the group heading beside it reads as a heading and
  // not as an entry that lost its icon — a distinction that is only worth
  // making if both ends of it stay legible.
  ['--text-nav', '--bg-surface'],
  ['--text-muted', '--bg-surface'],
];

/** Badge text on its own translucent background, itself over a card. */
const BADGES = ['success', 'warning', 'danger', 'info'];

/**
 * A token the palette must declare.
 *
 * The lookup can miss — a renamed custom property, a block this file's regex no
 * longer matches — and the useful failure names the token. Left to flow through
 * as `undefined`, it would surface as `unsupported colour: undefined` several
 * calls away, which says nothing about which one went missing.
 */
function token(palette: Record<string, string>, name: string): string {
  const value = palette[name];
  if (value === undefined) throw new Error(`the palette declares no ${name}`);
  return value;
}

/**
 * Read from the two blocks rather than from the merge: what has to match is
 * what each one *states*, since a token missing from either falls through to
 * the dark value rather than to nothing.
 */
it('states the light palette identically for the explicit theme and for auto', () => {
  const explicit = Object.keys(tokensOf(/^:root\[data-theme='light'\]\s*\{([\s\S]*?)\n\}/m));
  const stamped = Object.keys(tokensOf(AUTO));
  expect(stamped.sort()).toEqual(explicit.sort());
});

describe.each([
  ['dark', dark],
  ['light', light],
  ['auto light', auto],
])('the %s palette', (name, palette) => {
  it('parses into a usable set of tokens', () => {
    expect(Object.keys(palette).length).toBeGreaterThan(10);
    expect(palette['--bg-base']).toBeTruthy();
  });

  it.each(PAIRS)('reads %s on %s at AA', (fg, bg) => {
    const ratio = contrast(token(palette, fg), token(palette, bg));
    expect(ratio, `${name}: ${fg} on ${bg} is ${ratio.toFixed(2)}:1`).toBeGreaterThanOrEqual(4.5);
  });

  it.each(BADGES)('reads %s badge text at AA', (status) => {
    const background = over(token(palette, `--status-${status}-bg`), token(palette, '--bg-card'));
    const ratio = contrast(token(palette, `--status-${status}`), background);
    expect(
      ratio,
      `${name}: --status-${status} on its badge is ${ratio.toFixed(2)}:1`,
    ).toBeGreaterThanOrEqual(4.5);
  });

  /**
   * WCAG 1.4.11: the boundary that identifies a control needs 3:1, not 4.5.
   *
   * `--border-subtle` measures 1.37:1 against `--bg-input` — a field frame
   * that is a suggestion rather than an edge, and the reason fields were only
   * identifiable by hovering them. `--border-strong` is what every input and
   * secondary button wears instead; passive separations keep the subtle one,
   * since a card edge identifies nothing.
   */
  it('draws the boundary of a control at 3:1', () => {
    const ratio = contrast(token(palette, '--border-strong'), token(palette, '--bg-input'));
    expect(
      ratio,
      `${name}: --border-strong on --bg-input is ${ratio.toFixed(2)}:1`,
    ).toBeGreaterThanOrEqual(3);
  });

  it('reads the primary button label on its fill', () => {
    // `.btn-primary` writes #111 on the accent, in both themes.
    const ratio = contrast('#111111', token(palette, '--accent-primary'));
    expect(ratio, `${name}: the button label is ${ratio.toFixed(2)}:1`).toBeGreaterThanOrEqual(4.5);
  });
});
