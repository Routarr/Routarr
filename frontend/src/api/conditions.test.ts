import fc from 'fast-check';
import { describe, expect, it } from 'vitest';
import type { ConditionSpec, LibraryFacets } from './types';
import {
  defaultConditionValue,
  parseNumberList,
  rejectedNumbers,
  parseStringList,
  parseScoreBound,
  parseWholeBound,
  canonicalKey,
  foldCertification,
  addValue,
  removeValue,
  withoutRepeats,
  conditionAppliesTo,
  localFacets,
  MAX_PRIORITY,
  nextPriority,
  foldStatus,
  issueField,
} from './conditions';
import { backendConstant, readBackend } from '../test/backend';

const spec = (value_type: ConditionSpec['value_type']): ConditionSpec => ({
  type: 'x',
  label: 'X',
  group: 'metadata',

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
  readBackend('backend/src/services/rule_engine/normalise_value_cases.json'),
) as [string, string][];

describe('canonicalKey', () => {
  it.each(FOLDING)('folds %j as the rule engine does', (raw, folded) => {
    expect(canonicalKey(raw)).toBe(folded);
  });
});

/** The rating codes, keyed as the rule engine keys them. */
const RATINGS: [string, string][] = JSON.parse(
  readBackend('backend/src/services/rule_engine/certification_key_cases.json'),
) as [string, string][];

describe('foldCertification', () => {
  it.each(RATINGS)('keys %j as the rule engine does', (raw, key) => {
    expect(foldCertification(raw)).toBe(key);
  });

  it('keeps R+ apart from R in a rating condition, and only there', () => {
    expect(withoutRepeats({ type: 'certification_in', value: ['R', 'R+'] }).value).toEqual([
      'R',
      'R+',
    ]);
    expect(addValue(['R'], 'R+', foldCertification)).toEqual(['R', 'R+']);
    expect(addValue(['PG-13'], 'pg 13', foldCertification)).toEqual(['PG-13']);
    expect(withoutRepeats({ type: 'genre_contains', value: ['Sci-Fi', 'sci fi'] }).value).toEqual([
      'Sci-Fi',
    ]);
  });
});

describe('foldStatus', () => {
  it('reads a status as one word, as the rule engine does', () => {
    expect(foldStatus('In Cinemas')).toBe(foldStatus('inCinemas'));
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

  /** The free-text path has to produce exactly what the picker would. */
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

  /** The server refuses the rest, a fraction with the whole request. */
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

describe('parseWholeBound', () => {
  it('clears the bound when the field is emptied', () => {
    // Sending 0 instead of null would turn an open range into "year >= 0".
    expect(parseWholeBound('')).toBeNull();
    expect(parseWholeBound('   ')).toBeNull();
  });

  it('parses a year', () => {
    expect(parseWholeBound('1988')).toBe(1988);
  });

  it('rejects nonsense rather than storing NaN', () => {
    expect(parseWholeBound('nineteen')).toBeNull();
  });

  /** `Number` calls both finite, and either one stored is a rule that matches nothing. */
  it('takes a whole number or nothing', () => {
    expect(parseWholeBound('2e5')).toBeNull();
    expect(parseWholeBound('1988.5')).toBeNull();
    expect(parseWholeBound(' 1988 ')).toBe(1988);
  });
});

describe('parseScoreBound', () => {
  it('reads a fraction written with a point or a comma, and nothing else', () => {
    expect(parseScoreBound('7.5')).toBe(7.5);
    expect(parseScoreBound(' 7,5 ')).toBe(7.5);
    expect(parseScoreBound('94')).toBe(94);
    expect(parseScoreBound('')).toBeNull();
    expect(parseScoreBound('-1')).toBeNull();
    expect(parseScoreBound('1e2')).toBeNull();
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
    // The union would offer a condition that silently makes half the rule dead.
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
    expect(nextPriority([MAX_PRIORITY - 5])).toBe(MAX_PRIORITY);
  });

  it('holds a priority to the ceiling the rule engine refuses past', () => {
    expect(MAX_PRIORITY).toBe(
      backendConstant('backend/src/services/rule_engine/validation.rs', 'MAX_PRIORITY'),
    );
  });
});

describe('the parsers of a typed value', () => {
  /**
   * Whatever is typed, a parser answers, and its answer typed back reads the same: an identifier
   * is a whole number a source can hold, and a year one that reads back as itself.
   */
  it('read any input, and settle on their own output', () => {
    const whole = fc.bigInt({ min: -(10n ** 30n), max: 10n ** 30n }).map(String);
    const typed = fc.oneof(
      fc.string(),
      fc.stringMatching(/^[-\d ,]{0,40}$/),
      fc.array(whole, { maxLength: 5 }).map((parts) => parts.join(', ')),
    );
    fc.assert(
      fc.property(typed, (input) => {
        const strings = parseStringList(input);
        expect(parseStringList(strings.join(', '))).toEqual(strings);

        const ids = parseNumberList(input);
        expect(ids.every((id) => Number.isSafeInteger(id) && id > 0)).toBe(true);
        expect(parseNumberList(ids.join(', '))).toEqual(ids);

        const year = parseWholeBound(input);
        if (year !== null) expect(parseWholeBound(String(year))).toBe(year);
      }),
      // A zero with its sign, and digits past an exact integer.
      { examples: [['-0'], ['123456789012345678901234567890']] },
    );
  });
});

describe('issueField', () => {
  /** The server counts conditions from 1, the editor's rows from 0. */
  it('names the row a condition issue places, else the list, else nothing', () => {
    const at = (field: string, params: Record<string, string> = {}) =>
      issueField({ field, params });
    expect(at('name')).toBe('rules-rule-name');
    expect(at('exclusions', { index: '2' })).toBe('rules-exclusions-1');
    expect(at('conditions')).toBe('rules-conditions-add');
    expect(at('conditions', { index: '0' })).toBe('rules-conditions-add');
    expect(at('instance_ids')).toBeNull();
  });
});
