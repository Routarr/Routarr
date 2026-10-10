import { describe, it, expect } from 'vitest';
import fs from 'node:fs';
import path from 'node:path';

import { CARD_HELP, HELP, TERMS, glossary, helpScreen, searchHelp } from './help';
import { SCREENS } from './routes';
import { SECTIONS } from './settings';

/**
 * Every screen, every card and every term has its help, in the dictionaries,
 * and nothing the help names is missing or left over: a screen added without
 * its help, a card drawn without its "?" text, or a term no screen uses fails
 * here rather than reaching a reader as a bare key.
 */

const SRC = path.resolve(__dirname, '..');
const english = JSON.parse(
  fs.readFileSync(path.join(SRC, '..', '..', 'backend', 'locales', 'en.json'), 'utf-8'),
) as Record<string, string>;

/** Every `.svelte` file of the interface, as text. */
function sources(): string[] {
  const out: string[] = [];
  const walk = (dir: string) => {
    for (const entry of fs.readdirSync(dir, { withFileTypes: true })) {
      const at = path.join(dir, entry.name);
      if (entry.isDirectory()) walk(at);
      else if (entry.name.endsWith('.svelte')) out.push(fs.readFileSync(at, 'utf-8'));
    }
  };
  walk(SRC);
  return out;
}

describe('the help', () => {
  it('has an entry for every screen, and for nothing else', () => {
    expect(Object.keys(HELP).sort()).toEqual([...SCREENS].sort());
  });

  it('gives every screen its terms and the problems met there', () => {
    const thin = Object.entries(HELP)
      .filter(([, help]) => help.terms.length === 0 || help.problems.length === 0)
      .map(([path]) => path);
    expect(thin).toEqual([]);
  });

  it('names only keys the English dictionary holds', () => {
    const keys = [
      ...Object.values(HELP).flatMap((help) => [
        help.purpose,
        ...help.problems.flatMap((problem) => [problem.problem, problem.fix]),
      ]),
      ...Object.values(TERMS).flatMap((term) => [term.name, term.text]),
      ...Object.values(CARD_HELP).flatMap((card) => [card.title, card.text]),
    ];
    expect(keys.filter((key) => !(key in english))).toEqual([]);
  });

  it('defines every term a screen uses, and uses every term it defines', () => {
    const used = new Set(Object.values(HELP).flatMap((help) => help.terms));
    expect([...used].filter((id) => !(id in TERMS))).toEqual([]);
    expect(Object.keys(TERMS).filter((id) => !used.has(id))).toEqual([]);
  });

  it('lists every card on a screen, and holds a text for every card listed', () => {
    const listed = new Set(Object.values(HELP).flatMap((help) => help.cards));
    expect([...listed].filter((id) => !(id in CARD_HELP))).toEqual([]);
    expect(Object.keys(CARD_HELP).filter((id) => !listed.has(id))).toEqual([]);
  });

  it('sends every fix to a screen that exists', () => {
    const targets = Object.values(HELP).flatMap((help) =>
      help.problems.flatMap((problem) => (problem.to ? [problem.to] : [])),
    );
    expect(targets.filter((to) => !SCREENS.includes(to))).toEqual([]);
  });

  /** Read from the markup, so a card drawn with an id the help does not know fails here. */
  it('holds the help of every card the interface draws', () => {
    const drawn = new Set<string>();
    for (const text of sources()) {
      for (const [, id] of text.matchAll(/<(?:CardTitle|HelpToggle)\b[^>]*\bcard="([^"]+)"/g)) {
        drawn.add(id!);
      }
    }
    // The cards whose id is computed: a group of the API reference, a tab of Settings.
    const reference = fs.readFileSync(path.join(SRC, 'pages', 'ApiReference.svelte'), 'utf-8');
    const tags = reference.match(/const TAGS[^{]*\{([^}]*)\}/)?.[1] ?? '';
    for (const [, key] of tags.matchAll(/'(ApiTag\w+)'/g)) drawn.add(key!);
    for (const section of SECTIONS) {
      if (section.id !== 'metadata') drawn.add(section.labelKey);
    }
    drawn.add('SettingsMetadataCoverage');

    expect(drawn.size).toBeGreaterThan(20);
    expect([...drawn].filter((id) => !(id in CARD_HELP)).sort()).toEqual([]);
  });
});

describe('helpScreen', () => {
  it('opens on the screen on display, and on the dashboard for an address no screen answers', () => {
    expect(helpScreen('/rules')).toBe('/rules');
    expect(helpScreen('/nowhere')).toBe('/');
  });
});

describe('searchHelp', () => {
  const words: Record<string, string> = {
    [TERMS[Object.keys(TERMS)[0]!]!.name]: 'Écran témoin',
  };
  const translate = (key: string) => words[key] ?? key;

  it('finds a word whatever its case and its accents', () => {
    const hits = searchHelp('ecran TEMOIN', translate);
    expect(hits.map((hit) => hit.title)).toContain('Écran témoin');
  });

  it('lists a term once, under the first screen that uses it', () => {
    const term = Object.keys(TERMS)[0]!;
    const hits = searchHelp('Écran témoin', translate).filter((hit) => hit.kind === 'term');
    expect(hits).toHaveLength(1);
    expect(hits[0]!.screen).toBe(SCREENS.find((screen) => HELP[screen]!.terms.includes(term)));
  });

  it('puts what a title holds before what only a text holds', () => {
    const term = TERMS[Object.keys(TERMS)[0]!]!;
    const said = { [term.name]: 'Zebra', [HELP['/']!.purpose]: 'A zebra in a sentence' };
    const hits = searchHelp('zebra', (key) => said[key] ?? key);
    expect(hits.map((hit) => hit.kind)).toEqual(['term', 'purpose']);
  });

  it('answers nothing to an empty query', () => {
    expect(searchHelp('   ', translate)).toEqual([]);
  });
});

describe('glossary', () => {
  it('lists every term in the reader alphabetical order, with the screens that use it', () => {
    const entries = glossary((key) => key, 'en');
    expect(entries).toHaveLength(Object.keys(TERMS).length);
    const names = entries.map((entry) => entry.name);
    expect(names).toEqual(
      [...names].sort(new Intl.Collator('en', { sensitivity: 'base' }).compare),
    );
    for (const entry of entries) {
      expect(entry.screens.length).toBeGreaterThan(0);
      expect(entry.screens.every((screen) => HELP[screen]!.terms.includes(entry.id))).toBe(true);
    }
  });
});
