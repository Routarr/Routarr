import { afterEach, describe, it, expect, vi } from 'vitest';

import { api } from '../api/client';

import { formatCount } from '../api/format';
import { SERVER_COUNTS } from '../test/counts';
import { loadDictionary, seedDictionary, startDictionary, stored, t } from './i18n.svelte';
import { readBackend } from '../test/backend';

describe('t', () => {
  /**
   * A value is text the reader typed or the library holds, a rule named after
   * a placeholder among them. Replaced in turn by the parameter after it, the
   * name would read as a count.
   */
  it('never replaces a placeholder that arrived inside a value', () => {
    seedDictionary({ RuleHolds: '{name} holds {count}' });

    expect(t('RuleHolds', { name: 'Rule {count}', count: 3 })).toBe('Rule {count} holds 3');
  });
});

describe('loadDictionary', () => {
  afterEach(() => vi.restoreAllMocks());

  /** `nb_NO` is a dictionary's name, not a language tag a browser reads. */
  it('marks the page with a language tag the browser reads', async () => {
    vi.spyOn(api, 'getLocalization').mockResolvedValue({
      language: 'nb_NO',
      direction: 'ltr',
      strings: {},
      counts: [],
      theme: 'dark',
    });

    await loadDictionary();

    expect(document.documentElement.lang).toBe('nb-NO');
  });
});

/** The theme is a server setting carried with the strings, so the first frame paints in it. */
describe('the theme the dictionary carries', () => {
  afterEach(() => {
    vi.restoreAllMocks();
    delete document.documentElement.dataset.theme;
  });

  it('is applied with the strings, and auto leaves the system to decide', async () => {
    const dictionary = { language: 'en', direction: 'ltr' as const, strings: {}, counts: [] };
    vi.spyOn(api, 'getLocalization')
      .mockResolvedValueOnce({ ...dictionary, theme: 'light' })
      .mockResolvedValueOnce({ ...dictionary, theme: 'auto' });

    await loadDictionary();
    expect(document.documentElement.dataset.theme).toBe('light');
    await loadDictionary();
    expect(document.documentElement.dataset.theme).toBeUndefined();
  });
});

describe('a count in a sentence', () => {
  /** Written as the language groups digits, as every size and percentage is. */
  it('is grouped the way the language writes numbers, and a year is not', () => {
    seedDictionary({ Tasks: 'Tasks: {count}', Since: 'Since {min}' }, 'fr', SERVER_COUNTS);

    expect(t('Tasks', { count: 12345 })).toBe('Tasks: 12 345');
    expect(t('Since', { min: 2026 })).toBe('Since 2026');
  });

  it('groups every placeholder the server names a count', async () => {
    vi.spyOn(api, 'getLocalization').mockResolvedValue({
      language: 'de',
      direction: 'ltr',
      strings: { Moved: 'Moved: {applied} of {requested}' },
      counts: ['applied'],
      theme: 'dark',
    });
    await loadDictionary();

    expect(t('Moved', { applied: 12345, requested: 12345 })).toBe('Moved: 12.345 of 12345');
    vi.restoreAllMocks();
  });

  /**
   * The server groups the counts of the sentences it writes through its own
   * table, held to the same cases in `localization.rs`.
   */
  it('is grouped as the server groups it, in every language', () => {
    const cases = JSON.parse(readBackend('backend/src/grouped_count_cases.json')) as {
      numbers: number[];
      grouped: Record<string, string[]>;
    };
    for (const [language, grouped] of Object.entries(cases.grouped)) {
      expect(
        cases.numbers.map((n) => formatCount(n, language)),
        language,
      ).toEqual(grouped);
    }
  });
});

describe('a path in a sentence', () => {
  afterEach(() => {
    vi.restoreAllMocks();
    seedDictionary({});
  });

  /**
   * In Arabic a path's leading slash would move to its end. In English the
   * marks would only be noise in what a reader copies.
   */
  it('keeps its own direction in a right-to-left sentence, and only there', async () => {
    for (const [language, direction, expected] of [
      ['ar', 'rtl', 'أضيفت ⁨/movies/anime⁩'],
      ['en', 'ltr', 'أضيفت /movies/anime'],
    ] as const) {
      vi.spyOn(api, 'getLocalization').mockResolvedValue({
        language,
        direction,
        strings: { Declared: 'أضيفت {path}' },
        counts: [],
        theme: 'dark',
      });
      await loadDictionary();

      expect(t('Declared', { path: '/movies/anime' })).toBe(expected);
    }
  });

  /** Left bare in Arabic, "2001: A Space Odyssey" reads "A Space Odyssey :2001". */
  it('isolates every value in a right-to-left sentence but a count', () => {
    seedDictionary(
      { ConfirmRevert: '{title} {path}', Counted: '{count} {name}' },
      'ar',
      ['count'],
      'rtl',
    );

    expect(t('ConfirmRevert', { title: '2001: A Space Odyssey', path: '/a' })).toBe(
      '⁨2001: A Space Odyssey⁩ ⁨/a⁩',
    );
    expect(t('Counted', { count: 3, name: 'Radarr' })).toBe('3 ⁨Radarr⁩');
  });

  /** A text a row keeps is compared and shown in either direction later: no mark goes in it. */
  it('substitutes a text to store without an isolate, a count still grouped', () => {
    seedDictionary({ Counted: '{count} {name}' }, 'ar', ['count'], 'rtl');

    expect(stored('Counted', { count: 12345, name: 'Radarr' })).toBe(
      `${formatCount(12345, 'ar')} Radarr`,
    );
  });
});

describe('the dictionary the page carries', () => {
  afterEach(() => {
    vi.restoreAllMocks();
    document.getElementById('dictionary')?.remove();
    seedDictionary({});
  });

  it('is read before anything is fetched', async () => {
    const fetched = vi.spyOn(api, 'getLocalization');
    const carried = document.createElement('script');
    carried.type = 'application/json';
    carried.id = 'dictionary';
    carried.textContent = JSON.stringify({
      language: 'fr',
      direction: 'ltr',
      strings: { Dashboard: 'Tableau de bord' },
      counts: [],
      theme: 'dark',
    });
    document.head.append(carried);

    await startDictionary();

    expect(t('Dashboard')).toBe('Tableau de bord');
    expect(document.documentElement.lang).toBe('fr');
    expect(fetched).not.toHaveBeenCalled();
  });

  it('is fetched when the page carries none', async () => {
    vi.spyOn(api, 'getLocalization').mockResolvedValue({
      language: 'de',
      direction: 'ltr',
      strings: { Dashboard: 'Übersicht' },
      counts: [],
      theme: 'dark',
    });

    await startDictionary();

    expect(t('Dashboard')).toBe('Übersicht');
  });
});
