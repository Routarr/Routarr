import { describe, expect, it, onTestFinished, vi } from 'vitest';
import { readBackend } from '../test/backend';
import {
  describeCondition,
  failureDetail,
  formatBytes,
  formatPercent,
  formatTimestamp,
  localName,
  mediaTypeKey,
  runsOf,
  swapped,
  toggled,
} from './format';

describe('failureDetail', () => {
  it('keeps the error text and drops the marker the backend puts before it', () => {
    expect(failureDetail('error: connection refused')).toBe('connection refused');
  });

  it('leaves a status without the marker as it is', () => {
    expect(failureDetail('timed out')).toBe('timed out');
  });
});

describe('mediaTypeKey', () => {
  it('names each kind of title by its dictionary key', () => {
    expect(mediaTypeKey('movie')).toBe('Movies');
    expect(mediaTypeKey('series')).toBe('Series');
  });
});

describe('localName', () => {
  /** The vocabulary the server sends is spelled in English, for every reader. */
  it("names a language and a region in the reader's language, with the code", () => {
    expect(localName('language', 'ja', 'fr')).toBe('japonais (ja)');
    expect(localName('region', 'JP', 'fr')).toBe('Japon (JP)');
    expect(localName('language', 'ja', 'en')).toBe('Japanese (ja)');
  });

  it('leaves a code the browser cannot name to the caller', () => {
    expect(localName('language', 'qaa', 'fr')).toBeNull();
  });

  /** TMDB still sends these, and the browser names each after its successor. */
  it('leaves a code that no longer exists to the caller', () => {
    for (const code of ['SU', 'DD', 'YU', 'CS', 'AN']) {
      expect(localName('region', code, 'en')).toBeNull();
    }
    expect(localName('region', 'RU', 'en')).toBe('Russia (RU)');
  });

  /** A shipped language code with a region is written with `_`, which Intl refuses. */
  it('names in a language whose code carries a region', () => {
    expect(localName('language', 'fr', 'nb_NO')).toBe('fransk (fr)');
    expect(localName('region', 'JP', 'zh_TW')).toBe('日本 (JP)');
  });
});

describe('formatBytes', () => {
  const plain = (value: string) => value.replace(/\u202f|\u00a0/g, ' ');
  const SIZES = JSON.parse(readBackend('backend/src/byte_cases.json')) as [
    number,
    string,
    string,
  ][];

  /**
   * Each size in each shipped language, named as the server names it, which
   * the same cases hold the server to. Binary steps and one decimal past
   * bytes, the unit and the decimal mark of the reader's language rather than
   * the source's, and its spacing: none in Korean. The short form spaces the
   * unit and the narrow form holds the abbreviation, and where `Intl` spells
   * the byte out (Dutch, Greek, Turkish, Korean, Traditional Chinese) the
   * symbol is the kilobyte's without its prefix. A language stored as
   * `nb_NO` is read as Norwegian, where `Intl` refuses the underscore.
   */
  it.each(SIZES)('names %d bytes in %s as %j', (bytes, language, named) => {
    expect(plain(formatBytes(bytes, language))).toBe(named);
  });

  it('stops at the largest unit it knows', () => {
    expect(formatBytes(1024 ** 6)).toContain('PB');
  });

  it('says nothing rather than zero when the Arr reported no size', () => {
    expect(formatBytes(null)).toBe('-');
    expect(formatBytes(undefined)).toBe('-');
    expect(formatBytes(-1)).toBe('-');
    expect(plain(formatBytes(0))).toBe('0 B');
  });
});

describe('describeCondition', () => {
  /**
   * Each condition reads as one sentence of the dictionary, its values filled
   * in, so no language glues a caption, a quantifier and a list together.
   * Stand-in sentences that name their key, so the key picked shows.
   */
  const words = {
    separator: ', ',
    empty: 'none',
    phrase: (key: string, params: Record<string, string> = {}) =>
      [key, ...Object.entries(params).map(([name, value]) => `${name}=${value}`)].join(' '),
  };

  it('fills a list condition sentence with its values', () => {
    expect(
      describeCondition({ type: 'genre_not_contains', value: ['Animation', 'Family'] }, words),
    ).toBe('ConditionPhraseGenreNotContains values=Animation, Family');
  });

  it('marks an empty list, which can never match', () => {
    expect(describeCondition({ type: 'genre_contains', value: [] }, words)).toBe(
      'ConditionPhraseGenreContains values=none',
    );
  });

  it('says a year range open at one end in a sentence of its own', () => {
    const year = (min: number | null, max: number | null) =>
      describeCondition({ type: 'year_range', value: { min, max } }, words);
    expect(year(1980, 1999)).toBe('ConditionPhraseYearRange min=1980 max=1999');
    expect(year(1980, null)).toBe('ConditionPhraseYearRangeFrom min=1980');
    expect(year(null, 1999)).toBe('ConditionPhraseYearRangeTo max=1999');
    expect(year(null, null)).toBe('year_range');
  });

  it('says a yes or no condition either way, and fills a number in', () => {
    expect(describeCondition({ type: 'has_files', value: true }, words)).toBe(
      'ConditionPhraseHasFiles',
    );
    expect(describeCondition({ type: 'has_files', value: false }, words)).toBe(
      'ConditionPhraseHasFilesNot',
    );
    expect(describeCondition({ type: 'added_within_days', value: 7 }, words)).toBe(
      'ConditionPhraseAddedWithinDays value=7',
    );
  });

  it('falls back on the caption for a missing value', () => {
    expect(describeCondition({ type: 'has_files' }, { ...words, label: 'Has files' })).toBe(
      'Has files',
    );
  });

  /** The engine's identifiers never reach the reader when a name exists. */
  it('names the values and joins them as the language does', () => {
    const named = describeCondition(
      { type: 'original_language', value: ['ja', 'ko'] },
      {
        ...words,
        separator: '، ',
        name: (value) => (value === 'ja' ? 'Japanese (ja)' : value),
      },
    );

    expect(named).toBe('ConditionPhraseOriginalLanguage values=Japanese (ja)، ko');
  });
});

describe('formatTimestamp', () => {
  it('renders a stored UTC timestamp in the configured language', () => {
    // Same instant, two languages: month-first against day-first.
    const en = formatTimestamp('2026-08-22 14:05:57', 'en-US');
    const fr = formatTimestamp('2026-08-22 14:05:57', 'fr-FR');

    expect(en).toContain('8/22');
    expect(fr).toContain('22/08');
  });

  /**
   * Without the Z, a browser east of Greenwich would shift the hour and a sync
   * could appear to have run in the future. The zone is pinned east of
   * Greenwich: on a host running in UTC, local time and UTC agree, and the two
   * readings would match whichever one the code took.
   */
  it('treats the stored value as UTC rather than local time', () => {
    vi.stubEnv('TZ', 'Asia/Tokyo');
    onTestFinished(() => {
      vi.unstubAllEnvs();
    });

    const utc = formatTimestamp('2026-08-22 14:05:57', 'en-GB');
    const explicit = formatTimestamp('2026-08-22T14:05:57Z', 'en-GB');

    expect(explicit).toBe('22/08/2026, 23:05');
    expect(utc).toBe(explicit);
  });

  it('maps a Servarr language code onto a BCP-47 locale', () => {
    // nb_NO would throw as a locale: the underscore has to become a hyphen.
    expect(() => formatTimestamp('2026-08-22 14:05:57', 'nb_NO')).not.toThrow();
    expect(formatTimestamp('2026-08-22 14:05:57', 'nb_NO')).toContain('22');
  });

  it('shows the fallback rather than an empty cell when there is no value', () => {
    expect(formatTimestamp(null, 'en')).toBe('-');
    expect(formatTimestamp(undefined, 'en', 'never')).toBe('never');
  });

  it('returns an unparseable value unchanged instead of "Invalid Date"', () => {
    expect(formatTimestamp('not a date', 'en')).toBe('not a date');
  });
});

describe('formatPercent', () => {
  /**
   * French puts a non-breaking space before the sign and English does not,
   * which is why this goes through `Intl` rather than a template string.
   */
  it('writes the sign the way the language writes it', () => {
    expect(formatPercent(0.7, 'en')).toBe('70%');
    expect(formatPercent(0.7, 'fr').replace(/\u202f|\u00a0/g, ' ')).toBe('70 %');
  });

  /**
   * The underscore form is what the settings store, where BCP-47 wants a
   * hyphen, and an unfixed `zh_CN` throws rather than falling back.
   */
  it('accepts the stored locale form', () => {
    expect(formatPercent(0.45, 'zh_CN')).toBe('45%');
  });

  it('clamps and rounds rather than inventing precision', () => {
    expect(formatPercent(0, 'en')).toBe('0%');
    expect(formatPercent(1, 'en')).toBe('100%');
    expect(formatPercent(1.4, 'en')).toBe('100%');
    expect(formatPercent(-0.2, 'en')).toBe('0%');
    expect(formatPercent(null, 'en')).toBe('0%');
    expect(formatPercent(0.6349, 'en')).toBe('63%');
  });
});

describe('toggled', () => {
  it('adds what is ticked and takes out what is cleared, leaving the list it was given alone', () => {
    const scopes = ['read'];

    expect(toggled(scopes, 'operate', true)).toEqual(['read', 'operate']);
    expect(toggled(['read', 'operate'], 'read', false)).toEqual(['operate']);
    expect(scopes).toEqual(['read']);
  });
});

describe('swapped', () => {
  /** Priority order is the routing: a reorder moves one entry by one place, nothing else. */
  it('trades an entry with its neighbour and leaves the list it was given alone', () => {
    const order = ['arr', 'tmdb', 'omdb'];

    expect(swapped(order, 1, -1)).toEqual(['tmdb', 'arr', 'omdb']);
    expect(swapped(order, 1, 1)).toEqual(['arr', 'omdb', 'tmdb']);
    expect(order).toEqual(['arr', 'tmdb', 'omdb']);
  });

  it('refuses a move past either end rather than wrapping round', () => {
    expect(swapped(['arr', 'tmdb'], 0, -1)).toBeNull();
    expect(swapped(['arr', 'tmdb'], 1, 1)).toBeNull();
    expect(swapped([], 0, 1)).toBeNull();
  });

  /** A sparse array passes a bounds check and still reads `undefined`. */
  it('refuses a hole rather than writing it into the order', () => {
    expect(swapped(['arr', , 'omdb'], 0, 1)).toBeNull();
  });
});

describe('runsOf', () => {
  /** The arrows count the whole list, so each entry keeps its place in it. */
  it('groups neighbours that share a key, each with its index in the whole list', () => {
    const runs = runsOf(['nav', 'nav', 'media', 'nav'], (kind) => kind);

    expect(runs).toEqual([
      {
        key: 'nav',
        entries: [
          { item: 'nav', index: 0 },
          { item: 'nav', index: 1 },
        ],
      },
      { key: 'media', entries: [{ item: 'media', index: 2 }] },
      { key: 'nav', entries: [{ item: 'nav', index: 3 }] },
    ]);
  });

  /** A value with no group is a run of its own kind, drawn bare. */
  it('keeps an absent key as a key of its own', () => {
    const runs = runsOf(
      [{ group: undefined }, { group: 'PG' }, { group: undefined }],
      (option) => option.group,
    );

    expect(runs.map((run) => run.key)).toEqual([undefined, 'PG', undefined]);
  });

  it('answers an empty list with no run', () => {
    expect(runsOf([], (item) => item)).toEqual([]);
  });
});
