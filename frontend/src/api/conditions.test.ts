import { describe, expect, it } from 'vitest';
import type { ConditionSpec } from './types';
import {
  defaultConditionValue,
  parseNumberList,
  parseStringList,
  parseYearBound,
  canonicalKey,
  addValue,
  removeValue,
  conditionAppliesTo,
} from './conditions';

const spec = (value_type: ConditionSpec['value_type']): ConditionSpec => ({
  type: 'x',
  label: 'X',
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

describe('canonicalKey', () => {
  /// The same folding the rule engine applies, so the interface never treats as
  /// distinct two values a rule would match alike.
  it('folds case, accents and separators', () => {
    expect(canonicalKey('  Science-Fiction ')).toBe('science fiction');
    expect(canonicalKey('Science Fiction')).toBe('science fiction');
    expect(canonicalKey('Comédie')).toBe('comedie');
  });

  it('does not invent synonyms', () => {
    expect(canonicalKey('Sci-Fi')).not.toBe(canonicalKey('Science Fiction'));
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

  /// A comma separates alternatives and never means "and": the free-text path
  /// has to produce exactly what the picker would.
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

  /// `Number` calls both of these finite, and either one stored is a rule that
  /// matches nothing.
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
