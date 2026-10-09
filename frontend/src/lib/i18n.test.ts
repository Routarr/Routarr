import { afterEach, describe, it, expect, vi } from 'vitest';

import { api } from '../api/client';
import { readFileSync } from 'node:fs';
import { join } from 'node:path';

import { formatCount } from '../api/format';
import { SERVER_COUNTS } from '../test/counts';
import { loadDictionary, seedDictionary, t } from './i18n.svelte';

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
      isolated: [],
    });

    await loadDictionary();

    expect(document.documentElement.lang).toBe('nb-NO');
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
      isolated: [],
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
    const cases = JSON.parse(
      readFileSync(join(process.cwd(), '../backend/src/grouped_count_cases.json'), 'utf8'),
    ) as { numbers: number[]; grouped: Record<string, string[]> };
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
        isolated: ['path'],
      });
      await loadDictionary();

      expect(t('Declared', { path: '/movies/anime' })).toBe(expected);
    }
  });
});
