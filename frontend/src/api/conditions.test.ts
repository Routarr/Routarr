import { readFileSync } from 'node:fs';
import { join } from 'node:path';
import { describe, expect, it } from 'vitest';
import type { ConditionSpec, LibraryFacets } from './types';
import {
  defaultConditionValue,
  parseNumberList,
  rejectedNumbers,
  parseStringList,
  parseYearBound,
  canonicalKey,
  certificationKey,
  addValue,
  removeValue,
  withoutRepeats,
  conditionAppliesTo,
  localFacets,
  nextPriority,
  statusKey,
} from './conditions';

const spec = (value_type: ConditionSpec['value_type']): ConditionSpec => ({
  type: 'x',
  label: 'X',
  label_key: 'ConditionLabelX',
  value_type,
  needs_metadata: false,
  media_types: ['movie', 'series'],
  suggestions: '',
  quantifier: '',
  counterpart: '',
  metadata_field: null,
  available: true,
});

describe('defaultConditionValue', () => {
  it('starts every value type in a shape the backend accepts', () => {
    expect(defaultConditionValue(spec('boolean'))).toBe(true);
    expect(defaultConditionValue(spec('number'))).toBe(7);
    expect(defaultConditionValue(spec('number_list'))).toEqual([]);
    expect(defaultConditionValue(spec('string'))).toBe('');
    expect(defaultConditionValue(spec('string_list'))).toEqual([]);
    expect(defaultConditionValue(spec('year_range'))).toEqual({ min: null, max: null });
  });
});

/**
 * The same folding the rule engine applies, read from the table its own test
 * reads: a pair one side joins and the other keeps apart is a second spelling
 * the picker refuses, or a pair the editor accepts and the server refuses.
 */
const FOLDING: [string, string][] = JSON.parse(
  readFileSync(
    join(process.cwd(), '../backend/src/services/rule_engine/normalise_value_cases.json'),
    'utf8',
  ),
) as [string, string][];

describe('canonicalKey', () => {
  it.each(FOLDING)('folds %j as the rule engine does', (raw, folded) => {
    expect(canonicalKey(raw)).toBe(folded);
  });
});

/** The rating codes, keyed as the rule engine keys them. */
const RATINGS: [string, string][] = JSON.parse(
  readFileSync(
    join(process.cwd(), '../backend/src/services/rule_engine/certification_key_cases.json'),
    'utf8',
  ),
) as [string, string][];

describe('certificationKey', () => {
  it.each(RATINGS)('keys %j as the rule engine does', (raw, key) => {
    expect(certificationKey(raw)).toBe(key);
  });

  it('keeps R+ apart from R in a rating condition, and only there', () => {
    expect(withoutRepeats({ type: 'certification_in', value: ['R', 'R+'] }).value).toEqual([
      'R',
      'R+',
    ]);
    expect(addValue(['R'], 'R+', certificationKey)).toEqual(['R', 'R+']);
    expect(addValue(['PG-13'], 'pg 13', certificationKey)).toEqual(['PG-13']);
    expect(withoutRepeats({ type: 'genre_contains', value: ['Sci-Fi', 'sci fi'] }).value).toEqual([
      'Sci-Fi',
    ]);
  });
});

describe('statusKey', () => {
  it('reads a status as one word, as the rule engine does', () => {
    expect(statusKey('In Cinemas')).toBe(statusKey('inCinemas'));
    expect(withoutRepeats({ type: 'status_is', value: ['inCinemas', 'In Cinemas'] }).value).toEqual(
      ['inCinemas'],
    );
  });
});

describe('addValue and removeValue', () => {
  it('refuses a value already there under another spelling', () => {
    expect(addValue(['Science Fiction'], 'science-fiction')).toEqual(['Science Fiction']);
    expect(addValue(['Animation'], 'Family')).toEqual(['Animation', 'Family']);
  });

  it('removes whichever spelling is stored', () => {
    expect(removeValue(['Science Fiction'], 'science-fiction')).toEqual([]);
  });

  it('ignores a value that is only punctuation', () => {
    expect(addValue([], '  ,  ')).toEqual([]);
  });
});

describe('parseStringList', () => {
  it('trims each entry', () => {
    expect(parseStringList('Animation ,  Family')).toEqual(['Animation', 'Family']);
  });

  /**
   * A comma separates alternatives and never means "and": the free-text path
   * has to produce exactly what the picker would.
   */
  it('drops a value repeated under another spelling', () => {
    expect(parseStringList('Animation, animation , Family')).toEqual(['Animation', 'Family']);
  });

  it('drops the blanks left by a trailing comma', () => {
    expect(parseStringList('Animation, ,')).toEqual(['Animation']);
    expect(parseStringList('')).toEqual([]);
    expect(parseStringList('   ')).toEqual([]);
  });

  it('keeps values containing spaces intact', () => {
    expect(parseStringList('studio ghibli, science fiction')).toEqual([
      'studio ghibli',
      'science fiction',
    ]);
  });
});

describe('parseNumberList', () => {
  it('parses identifiers', () => {
    expect(parseNumberList('8392, 129')).toEqual([8392, 129]);
  });

  it('discards fragments that are not numbers', () => {
    // A half-typed entry must not become NaN in the stored rule.
    expect(parseNumberList('8392, abc, 129')).toEqual([8392, 129]);
    expect(parseNumberList('')).toEqual([]);
  });

  /**
   * An identifier is a whole number above zero, listed once. The server
   * refuses the rest, a fraction with the whole request.
   */
  it('keeps whole numbers above zero, once each', () => {
    expect(parseNumberList('8392, 1.5, -3, 0, 8392')).toEqual([8392]);
  });

  /** `Number` reads a hexadecimal or an exponent as a number nobody typed. */
  it('takes digits only, and names what it left out', () => {
    expect(parseNumberList('603, 0x25B, 1e3, 6O4')).toEqual([603]);
    expect(rejectedNumbers('603, 0x25B, 1e3, 6O4, , 0')).toEqual(['0x25B', '1e3', '6O4', '0']);
    expect(rejectedNumbers('603, 604')).toEqual([]);
  });
});

describe('parseYearBound', () => {
  it('clears the bound when the field is emptied', () => {
    // Sending 0 instead of null would turn an open range into "year >= 0".
    expect(parseYearBound('')).toBeNull();
    expect(parseYearBound('   ')).toBeNull();
  });

  it('parses a year', () => {
    expect(parseYearBound('1988')).toBe(1988);
  });

  it('rejects nonsense rather than storing NaN', () => {
    expect(parseYearBound('nineteen')).toBeNull();
  });

  /**
   * `Number` calls both of these finite, and either one stored is a rule that
   * matches nothing.
   */
  it('takes a whole number or nothing', () => {
    expect(parseYearBound('2e5')).toBeNull();
    expect(parseYearBound('1988.5')).toBeNull();
    expect(parseYearBound(' 1988 ')).toBe(1988);
  });
});

describe('conditionAppliesTo', () => {
  const scoped = (media_types: string[]) => ({ ...spec('string_list'), media_types });

  it('offers a condition on the type that carries it', () => {
    expect(conditionAppliesTo(scoped(['series']), 'series')).toBe(true);
    expect(conditionAppliesTo(scoped(['movie', 'series']), 'movie')).toBe(true);
  });

  it('withholds one the type cannot carry', () => {
    // Radarr reports no season list, so this can only ever fail on a film.
    expect(conditionAppliesTo(scoped(['series']), 'movie')).toBe(false);
  });

  it('takes the intersection for a rule that covers both', () => {
    // A `both` rule is evaluated against films *and* series, so a series-only
    // condition can never hold for the films it also matches. The union would
    // offer a condition that silently makes half the rule dead.
    expect(conditionAppliesTo(scoped(['series']), 'both')).toBe(false);
    expect(conditionAppliesTo(scoped(['movie', 'series']), 'both')).toBe(true);
  });
});

describe('localFacets', () => {
  const facets = {
    vocabularies: {
      original_languages: [{ value: 'ja', label: 'Japanese (ja)', count: 0 }],
      origin_countries: [{ value: 'JP', label: 'Japan (JP)', count: 0 }],
    },
    original_languages: [{ value: 'ja', count: 9 }],
    origin_countries: [{ value: 'JP', label: 'Japan (JP)', count: 4 }],
    genres: [{ value: 'Animation', count: 12 }],
  } as unknown as LibraryFacets;

  it("names the languages and the regions in the reader's language, and nothing else", () => {
    const named = localFacets(facets, 'fr');

    expect(named.vocabularies.original_languages[0]?.label).toBe('japonais (ja)');
    expect(named.vocabularies.origin_countries[0]?.label).toBe('Japon (JP)');
    expect(named.original_languages[0]?.label).toBe('japonais (ja)');
    expect(named.origin_countries[0]?.label).toBe('Japon (JP)');
    expect(named.genres).toBe(facets.genres);
  });
});

describe('nextPriority', () => {
  it('places a new rule after every other, as the server does', () => {
    expect(nextPriority([30, 10])).toBe(40);
    expect(nextPriority([])).toBe(10);
  });
});
