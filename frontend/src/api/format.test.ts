import { describe, expect, it, onTestFinished, vi } from 'vitest';
import { readBackend } from '../test/backend';
import {
  bcp47,
  expiresWithinAWeek,
  idleFor90Days,
  dayStart,
  formatDecimal,
  setFields,
  describeCondition,
  formatBytes,
  formatPercent,
  formatTimestamp,
  inClearPastHome,
  localName,
  mediaTypeKey,
  runsOf,
  swapped,
  toggled,
} from './format';

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
   * Each size in each shipped language as the server names it, from the cases that hold the
   * server: binary steps, one decimal past bytes, the reader's unit, decimal mark and spacing.
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
   * Each condition reads as one dictionary sentence, so no language glues a caption, a quantifier
   * and a list together. The stand-in sentences name their key, so the key picked shows.
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

  it('names a score scale, writes its numbers as the language does and adds its votes', () => {
    const scored = {
      ...words,
      scale: (id: string) => id.toUpperCase(),
      number: (n: number) => `<${n}>`,
    };
    const score = (value: Record<string, unknown>) =>
      describeCondition({ type: 'score_range', value }, scored);
    expect(score({ scale: 'imdb', min: 7.5, max: 10, min_votes: null })).toBe(
      'ConditionPhraseScoreRange scale=IMDB min=<7.5> max=<10>',
    );
    expect(score({ scale: 'imdb', min: 7.5, max: null, min_votes: 1000 })).toBe(
      'ConditionPhraseScoreRangeFrom scale=IMDB min=<7.5>, ScoreVotesAtLeast votes=1000',
    );
    expect(score({ scale: 'anilist', min: null, max: 40 })).toBe(
      'ConditionPhraseScoreRangeTo scale=ANILIST max=<40>',
    );
    // Without the catalogue's names nor the language's numbers, the stored ones stand in.
    expect(
      describeCondition({ type: 'score_range', value: { scale: 'mal', min: 8, max: null } }, words),
    ).toBe('ConditionPhraseScoreRangeFrom scale=mal min=8');
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
  /**
   * A sentence the server writes names a day through its own table of patterns, held to the
   * date this helper writes by the cases `localization.rs` reads too.
   */
  it('writes its date as the server writes one, in every language', () => {
    const cases = JSON.parse(readBackend('backend/src/date_cases.json')) as {
      dates: string[];
      written: Record<string, string[]>;
    };
    for (const [language, written] of Object.entries(cases.written)) {
      const short = new Intl.DateTimeFormat(bcp47(language), {
        dateStyle: 'short',
        timeZone: 'UTC',
      });
      const days = cases.dates.map((day) => short.format(new Date(`${day}T12:00:00Z`)));
      expect(days, language).toEqual(written);
    }
  });

  it('renders a stored UTC timestamp in the configured language', () => {
    // Same instant, two languages: month-first against day-first.
    const en = formatTimestamp('2026-08-22 14:05:57', 'en-US');
    const fr = formatTimestamp('2026-08-22 14:05:57', 'fr-FR');

    expect(en).toContain('8/22');
    expect(fr).toContain('22/08');
  });

  /**
   * Without the Z, a browser east of Greenwich shifts the hour. The zone is pinned there, since on
   * a UTC host both readings match whichever one the code took.
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
  /** French puts a non-breaking space before the sign, which a template string would not. */
  it('writes the sign the way the language writes it', () => {
    expect(formatPercent(0.7, 'en')).toBe('70%');
    expect(formatPercent(0.7, 'fr').replace(/\u202f|\u00a0/g, ' ')).toBe('70 %');
  });

  /** The settings store `zh_CN`, which `Intl` throws on rather than falling back. */
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

describe('inClearPastHome', () => {
  it('stays quiet for an address that stays home or is encrypted', () => {
    for (const address of [
      'http://radarr:7878',
      'http://localhost:7878',
      'http://192.168.1.20:7878',
      'http://10.0.0.5:8989',
      'http://172.20.0.3:7878',
      'http://100.101.102.103:7878',
      'http://[fd00::5]:7878',
      'http://nas.lan:7878',
      'http://radarr.home.arpa',
      'https://radarr.example.com',
      'not an address',
    ]) {
      expect(inClearPastHome(address), address).toBe(false);
    }
  });

  it('names http to a public address or a public name', () => {
    for (const address of [
      'http://203.0.113.9:7878',
      'http://radarr.example.com',
      'http://user:pass@radarr.example.com/radarr',
      'http://[2001:db8::1]:7878',
      'http://172.32.0.1',
    ]) {
      expect(inClearPastHome(address), address).toBe(true);
    }
  });
});

describe('setFields', () => {
  it('leaves out a field nobody set, which would narrow to nothing', () => {
    expect(setFields({ instance_id: 'i1', category: '', search: 'aki' })).toEqual({
      instance_id: 'i1',
      search: 'aki',
    });
  });
});

describe('dayStart', () => {
  const local = (instant: string | undefined) => {
    const at = new Date(instant ?? NaN);
    return [at.getFullYear(), at.getMonth() + 1, at.getDate(), at.getHours(), at.getMinutes()];
  };

  it('is the local midnight a day starts at, and with `after` the next one', () => {
    expect(local(dayStart('2026-10-18'))).toEqual([2026, 10, 18, 0, 0]);
    expect(local(dayStart('2026-10-18', true))).toEqual([2026, 10, 19, 0, 0]);
    expect(local(dayStart('2026-12-31', true))).toEqual([2027, 1, 1, 0, 0]);
  });

  it('gives nothing for no day', () => {
    expect(dayStart('')).toBeUndefined();
    expect(dayStart('not a day')).toBeUndefined();
  });
});

describe('formatDecimal', () => {
  it('writes a fraction as the language does, a whole one with the digits asked', () => {
    expect(formatDecimal(7.5, 'fr')).toBe('7,5');
    expect(formatDecimal(7.5, 'en')).toBe('7.5');
    expect(formatDecimal(86, 'en')).toBe('86');
    expect(formatDecimal(8, 'fr', 1)).toBe('8,0');
  });
});

describe('expiresWithinAWeek', () => {
  const now = Date.parse('2026-10-19T12:00:00Z');
  it('holds for a key stopping within seven days, and for no other', () => {
    expect(expiresWithinAWeek('2026-10-26T12:00:00Z', now)).toBe(true);
    expect(expiresWithinAWeek('2026-10-26T12:00:01Z', now)).toBe(false);
    expect(expiresWithinAWeek('2026-10-19T11:59:59Z', now)).toBe(false);
    expect(expiresWithinAWeek(null, now)).toBe(false);
  });
});

describe('idleFor90Days', () => {
  const now = Date.parse('2026-10-19T12:00:00Z');
  it('counts from the last call, or from the making of a key never used', () => {
    expect(idleFor90Days('2026-07-21T12:00:00Z', '2026-01-01T00:00:00Z', now)).toBe(true);
    expect(idleFor90Days('2026-07-21T12:00:01Z', '2026-01-01T00:00:00Z', now)).toBe(false);
    expect(idleFor90Days(null, '2026-07-21T12:00:00Z', now)).toBe(true);
    expect(idleFor90Days(null, '2026-10-01T00:00:00Z', now)).toBe(false);
  });
});
