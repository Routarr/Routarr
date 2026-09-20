import { describe, it, expect } from 'vitest';
import fs from 'node:fs';
import path from 'node:path';

const SRC = path.resolve(__dirname, '..');

function read(file: string): string {
  return fs.readFileSync(path.join(SRC, file), 'utf-8');
}

/**
 * Every screen and every shared component, by extension.
 *
 * The glob is load-bearing: a pattern matching no file leaves the four checks
 * below iterating over an empty list and passing without reading anything.
 * Components are included because half the forms live in them.
 */
function pages(): string[] {
  return ['pages', 'components'].flatMap((dir) =>
    fs
      .readdirSync(path.join(SRC, dir))
      .filter((f) => f.endsWith('.svelte'))
      .map((f) => path.join(dir, f)),
  );
}

/**
 * A translated label is routinely twice the length of its English original:
 * `Type` stays on one line while `Synchroniser toutes les (min)` wraps onto two,
 * and a row that top-aligns its children then puts the two inputs at different
 * heights. `.form-row` bottom-aligns them, so the wrapping is invisible.
 */
describe('a row of fields survives labels of different lengths', () => {
  it('the form-row helper bottom-aligns its controls and wraps when narrow', () => {
    const rule = read('index.css').match(/\.form-row \{[^}]*\}/)?.[0] ?? '';
    expect(rule).toContain('align-items: flex-end');
    expect(rule).toContain('flex-wrap: wrap');
  });

  it('no page lays out fields side by side with a bare flex row', () => {
    const offenders: string[] = [];

    for (const file of pages()) {
      const lines = read(file).split('\n');
      lines.forEach((line, index) => {
        if (!/\bclass="flex gap-\d"/.test(line)) return;
        // A bare flex row is fine for buttons or badges; it is only wrong when
        // it carries labelled fields, whose labels wrap independently.
        const block = lines.slice(index + 1, index + 12).join('\n');
        if (block.includes('form-label')) {
          offenders.push(`${file}:${index + 1}`);
        }
      });
    }

    expect(offenders).toEqual([]);
  });

  /**
   * Split each `<button>` into its attributes and its body.
   *
   * Not a regular expression: `onclick={() => run()}` carries a `>` inside a
   * brace and `<button\b([^>]*?)>` stopped there, so the arrow landed in the
   * captured body and satisfied the "has visible text" test below. Every
   * icon-only button with an arrow handler — which is all of them — was exempt
   * from the check that names them. Tracking brace depth is what tells a `>`
   * closing the tag from a `>` inside an expression.
   */
  function buttons(source: string): { attributes: string; body: string }[] {
    const found: { attributes: string; body: string }[] = [];
    const OPEN = /<button\b/g;
    let opening: RegExpExecArray | null;
    while ((opening = OPEN.exec(source)) !== null) {
      let depth = 0;
      let cursor = opening.index + opening[0].length;
      while (cursor < source.length) {
        const char = source[cursor];
        if (char === '{') depth += 1;
        else if (char === '}') depth -= 1;
        else if (char === '>' && depth === 0) break;
        cursor += 1;
      }
      const close = source.indexOf('</button>', cursor);
      if (cursor >= source.length || close === -1) continue;
      found.push({
        attributes: source.slice(opening.index + opening[0].length, cursor),
        body: source.slice(cursor + 1, close),
      });
    }
    return found;
  }

  it('no button is left without an accessible name', () => {
    // An icon-only button announces nothing to a screen reader, and two of
    // these were destructive actions.
    const offenders: string[] = [];

    for (const file of pages()) {
      for (const { attributes, body } of buttons(read(file))) {
        // Control and render blocks are markup, not words: `{#if icon}` and
        // `{@render icon()}` name nobody. What is left is the button's text,
        // which may be a literal or an expression — `<span>{action.label}</span>`
        // is as much a name as `<span>{t('Save')}</span>`.
        const text = body.replace(/\{[#:/@][^}]*\}/g, '');
        // `\bt\(` and not `t(`: the latter is a substring of `split(` and of
        // every other call whose name ends in a t, which reads as a label.
        const hasVisibleText = /\bt\(/.test(text) || />\s*[A-Za-z{]/.test(text);
        const isLabelled = attributes.includes('aria-label') || attributes.includes('title=');
        if (!hasVisibleText && !isLabelled) {
          offenders.push(`${file}: ${body.replace(/\s+/g, ' ').trim().slice(0, 40)}`);
        }
      }
    }

    expect(offenders).toEqual([]);
  });

  it('every form label points at the control it describes', () => {
    // A `.form-label` sitting next to its input, with no `htmlFor`, looks right
    // and announces nothing: the field reads as unlabelled, and clicking the
    // caption does not focus it. Every one of them was in that state.
    const offenders: string[] = [];

    for (const file of pages()) {
      const source = read(file);
      for (const match of source.matchAll(/<label\b([^>]*class="form-label"[^>]*)>/g)) {
        const attributes = match[1];
        if (attributes !== undefined && !/\bfor=/.test(attributes)) {
          offenders.push(`${file}: ${match[0].replace(/\s+/g, ' ').slice(0, 60)}`);
        }
      }
    }

    expect(offenders).toEqual([]);
  });

  it('no two controls share an id', () => {
    // `htmlFor` resolves to the *first* match, so a duplicated id silently
    // points several labels at one control. A caption over a repeated component
    // is the easy way to create one.
    const seen = new Map<string, string>();
    const clashes: string[] = [];

    for (const file of pages()) {
      const source = read(file);
      for (const match of source.matchAll(/\bid="([a-z0-9-]+)"/g)) {
        const id = match[1];
        if (id === undefined) continue;
        const previous = seen.get(id);
        if (previous) clashes.push(`${id}: ${previous} and ${file}`);
        else seen.set(id, file);
      }
    }

    expect(clashes).toEqual([]);
  });

  it('a button keeps its label on one line whatever the language', () => {
    const rule = read('index.css').match(/\.btn \{[^}]*\}/)?.[0] ?? '';
    expect(rule).toContain('white-space: nowrap');
  });

  /**
   * A disabled button had no styling at all: full opacity, a pointer cursor,
   * and its hover fill still lighting up. Thirty controls looked clickable
   * while doing nothing, and every test that touched one asserted the
   * `disabled` attribute was set — none that anybody could tell.
   */
  it('a disabled button looks disabled', () => {
    const css = read('index.css');
    const rule = css.match(/\.btn:disabled[^{]*\{[^}]*\}/)?.[0] ?? '';
    expect(rule, 'no .btn:disabled rule at all').toContain('opacity');
    expect(rule).toContain('cursor: not-allowed');

    // And the hover fills must stand down, or the dimming is undone by the
    // first mouse that passes over it.
    const unguarded = [...css.matchAll(/\.btn-[a-z]+:hover(?::not\(:disabled\))?/g)]
      .map((m) => m[0])
      .filter((selector) => !selector.includes(':not(:disabled)'));
    expect(unguarded).toEqual([]);
  });

  /**
   * The form controls are drawn here, not by the browser.
   *
   * Removing the shared base rule merges its selectors into the focus rule
   * below, so every input falls back to the user agent — the platform's own
   * chrome — while wearing the accent border permanently. Nothing else asserts
   * that a text field has a background.
   */
  it('every field is drawn by this stylesheet rather than by the browser', () => {
    const css = read('index.css');

    const base = css.match(/\.form-input,\s*\.form-select,\s*\.form-textarea \{[^}]*\}/)?.[0] ?? '';
    for (const property of ['background-color', 'border:', 'border-radius', 'color:', 'padding']) {
      expect(base, `the shared field rule no longer sets ${property}`).toContain(property);
    }

    // And the accent border belongs to focus alone: on the base rule it makes
    // every field look permanently focused, which is how the regression looked.
    expect(base).not.toContain('--accent-strong');

    // The two controls the browser would otherwise draw itself.
    expect(css, 'selects fall back to the platform arrow').toMatch(
      /select\.form-(input|select)[^{]*\{[^}]*appearance: none/,
    );
    expect(css, 'the checkbox is left to the user agent').toMatch(
      /input\[type='checkbox'\] \{[^}]*appearance: none/,
    );

    // `color-scheme` is what the browser paints the select's own popup with.
    // Anchored to the start of a line, or `prefers-color-scheme` in the media
    // query counts as a fourth declaration.
    expect(
      css.match(/^\s*color-scheme:/gm) ?? [],
      'color-scheme is not set in all three theme blocks',
    ).toHaveLength(3);
  });

  it('page and card headers wrap their actions instead of crushing them', () => {
    const css = read('index.css');
    for (const selector of ['.page-header', '.card-header']) {
      const rule = css.match(new RegExp(`\\${selector} \\{[^}]*\\}`))?.[0] ?? '';
      expect(rule, selector).toContain('flex-wrap: wrap');
    }
  });
});

/**
 * The navigation and the route table, read as source and compared.
 *
 * Nothing else joins them: a route added to `App.svelte` with no entry in the
 * sidebar is a screen only a typed URL reaches, and an entry pointing at no
 * route lands on the dashboard without a word. Both have to be deliberate, and
 * this is what makes them so.
 */
/**
 * One search box, wherever a screen filters by typing.
 *
 * Four screens had three treatments — a magnifier at 18px on one, at 16px on
 * another, none at all on the third — and only one of them anchored its width,
 * so revealing a button beside the box shifted the whole row on the others.
 * Written out at each call site, that divergence is invisible to every other
 * check here: each screen is correct on its own.
 */
/**
 * The application's own voice, not the browser's.
 *
 * A native constraint bubble renders in the *browser's* language whatever
 * `ui_language` says, and fires before the submit handler — so it speaks over
 * whatever the form was going to say. Every form carrying one is `novalidate`
 * and holds its submit until the field is filled, which enforces the constraint
 * before the press instead of complaining about it after. jsdom runs no
 * constraint validation at all, so nothing but a read of the source sees this.
 */
describe('no form leaves the browser to do the talking', () => {
  const forms = () =>
    pages().flatMap((file) =>
      // The opening tag itself, not the file: the word also appears in the
      // comment above the forms saying why it is there.
      (read(file).match(/<form\b[^>]*>/g) ?? []).map((tag) => [file, tag] as const),
    );

  it('finds the forms at all', () => {
    // Empty, the check below would pass while reading nothing.
    expect(forms().length).toBeGreaterThanOrEqual(8);
  });

  it('marks every one of them novalidate', () => {
    for (const [file, tag] of forms()) {
      expect(tag.includes('novalidate'), `${file}: ${tag.slice(0, 44)}…`).toBe(true);
    }
  });
});

describe('every screen searches through the same box', () => {
  const searching = () =>
    pages().filter(
      (file) =>
        file !== 'components/SearchField.svelte' && /placeholder=\{t\('Search/.test(read(file)),
    );

  it('finds the screens that search at all', () => {
    // The list is what the check below iterates over: empty, it would pass
    // while reading nothing, which is the failure this file exists to avoid.
    expect(searching().length).toBeGreaterThanOrEqual(4);
  });

  it('routes each of them through SearchField rather than a bare input', () => {
    for (const file of searching()) {
      const source = read(file);
      expect(source, `${file} searches without SearchField`).toContain(
        "from '../components/SearchField.svelte'",
      );
      // A bare `<input>` carrying a search placeholder is the divergence
      // itself: it is what had no magnifier and no anchored width. The
      // magnifier on the overrides screen's submit button is a button's icon
      // and stays.
      expect(source, `${file} searches from a bare input`).not.toMatch(
        /<input(?:(?!\/?>)[\s\S])*?placeholder=\{t\('Search/,
      );
    }
  });
});

describe('the navigation covers the route table', () => {
  const routes = () =>
    [...read('App.svelte').matchAll(/^\s*'(\/[^']*)':/gm)].map((match) => match[1] as string);

  // The list moved out of the sidebar when a second reader appeared — the
  // command palette searches the same destinations the navigation draws — and
  // out of `navigation.ts` when a third did: the e2e sweeps read it as data.
  const navigable = () =>
    [...read('lib/routes.ts').matchAll(/to: '(\/[^']*)'/g)].map((match) => match[1] as string);

  it('reads both lists rather than an empty one', () => {
    expect(routes().length).toBeGreaterThan(10);
    expect(navigable().length).toBeGreaterThan(10);
  });

  it('gives every route exactly one entry, and every entry a route', () => {
    const inNav = navigable();
    expect([...inNav].sort()).toEqual([...routes()].sort());
    expect(new Set(inNav).size).toBe(inNav.length);
  });
});

/**
 * A class named in markup and defined nowhere.
 *
 * It is silent in every other check: the markup is valid, the component
 * renders, `svelte-check` types it and no test asserts a colour. What the
 * reader gets is the element with none of the styling it was written for —
 * `.stat-label` survived the deletion of the `.stat-card` block it belonged
 * to, so the explanation panel's two captions lost their muted 13px and were
 * set at body weight beside the values they name.
 *
 * jsdom applies no stylesheet, so nothing rendered can see this; only the
 * source can. Composed names (`is-{tone}`, `{active ? ' waiting' : ''}`) sit
 * inside an expression and are skipped — the fragments they build are static
 * strings in the same file, and a scoped `<style>` block counts as a
 * definition.
 */
describe('every class in the markup is a class that exists', () => {
  const CLASS = /\.(-?[_a-zA-Z][\w-]*)/g;

  /**
   * The classes a stylesheet defines, read from selector position alone.
   *
   * Not from the whole text: `.source-name` is named in a comment in the
   * component that uses it, and a set built by scanning everything counted that
   * mention as a definition — deleting the rule from `index.css` then left this
   * check green, which is the one thing it exists to refuse.
   */
  function selectors(stylesheet: string): Set<string> {
    const withoutComments = stylesheet.replace(/\/\*[\s\S]*?\*\//g, '');
    const names = [...withoutComments.matchAll(/([^{}]*)\{/g)].flatMap((rule) =>
      [...(rule[1] ?? '').matchAll(CLASS)].map((m) => m[1] ?? ''),
    );
    return new Set(names);
  }

  it('names no class that neither index.css nor the component defines', () => {
    const defined = selectors(read('index.css'));
    expect(defined.size).toBeGreaterThan(100);
    // The rules every check below leans on are really in there.
    expect(defined.has('source-name')).toBe(true);
    expect(defined.has('btn')).toBe(true);

    const orphans: string[] = [];
    for (const file of pages()) {
      const source = read(file);
      // Every style lives in `index.css` today, so this is empty — it is here
      // so a component that grows a `<style>` block is not reported wholesale.
      const scoped = selectors(
        [...source.matchAll(/<style[^>]*>([\s\S]*?)<\/style>/g)].map((m) => m[1] ?? '').join('\n'),
      );
      // Only the quoted literals: an attribute holding `{` is composed at
      // runtime and its parts are matched as literals elsewhere.
      for (const attribute of source.matchAll(/class="([^"{}]*)"/g)) {
        for (const token of (attribute[1] ?? '').split(/\s+/).filter(Boolean)) {
          if (!defined.has(token) && !scoped.has(token)) orphans.push(`${token} (${file})`);
        }
      }
    }

    expect(orphans).toEqual([]);
  });
});
