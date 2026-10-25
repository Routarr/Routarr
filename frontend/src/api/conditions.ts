// Coercion between the rule editor's inputs and the JSON the backend expects. A mistake here
// stores a rule that never matches, so it stays pure functions, tested apart from the form.

import type {
  Condition,
  ConditionSpec,
  Facet,
  FacetAxis,
  LibraryFacets,
  ValidationIssue,
  Vocabularies,
} from './types';
import { localName } from './format';

/** Value a freshly added condition starts with. */
export function defaultConditionValue(spec: ConditionSpec): unknown {
  switch (spec.value_type) {
    case 'boolean':
      return true;
    case 'number':
      return 7;
    case 'number_list':
      return [];
    case 'year_range':
    case 'number_range':
      return { min: null, max: null };
    case 'score_range':
      return { scale: 'imdb', min: null, max: null, min_votes: null };
    case 'string':
      return '';
    default:
      return [];
  }
}

/** The accents a decomposed Latin letter carries: `is_latin_accent` in the rule engine. */
const LATIN_ACCENT = /[\u0300-\u036f]/u;

/** The Latin letters as decomposition leaves them: `is_latin_letter`. */
const LATIN_LETTER = /[A-Za-z\u00c0-\u024f\u1e00-\u1eff]/u;

/** The letters whose mark does not decompose: `fold_stroke`. */
const STROKED: Record<string, string> = { ø: 'o', ł: 'l', đ: 'd', ħ: 'h', ı: 'i' };

/** What Rust's `char::is_alphanumeric` accepts, and a mark, which stays in its word. */
const WORD = /[\p{Alphabetic}\p{N}\p{M}]/u;

/**
 * The form two spellings of one value share: `normalise_value` in the rule engine, both held to
 * one table of cases. A Latin letter loses its accents, other scripts keep theirs (`ガ` is not
 * `カ`). It only tells values apart: what is stored and sent is the library's own spelling.
 */
export function canonicalKey(value: string): string {
  let latin = false;
  let unaccented = '';
  for (const c of value.normalize('NFKD')) {
    if (LATIN_ACCENT.test(c)) {
      if (!latin) unaccented += c;
      continue;
    }
    latin = LATIN_LETTER.test(c);
    unaccented += c;
  }
  let out = '';
  let gap = false;
  for (const c of unaccented.normalize('NFC')) {
    if (!WORD.test(c)) {
      gap = true;
      continue;
    }
    if (gap && out) out += ' ';
    gap = false;
    for (const lower of c.toLowerCase()) out += STROKED[lower] ?? lower;
  }
  return out;
}

/**
 * The form two spellings of one rating code share: `certification_key` in the
 * rule engine. Case and the separator aside, every character counts, so `R+`
 * stays apart from `R`. Exported for the tests, which hold it to the engine's cases.
 */
export function foldCertification(value: string): string {
  return value
    .trim()
    .replace(/[-_]/g, ' ')
    .replace(/[A-Z]/g, (letter) => letter.toLowerCase());
}

/**
 * An Arr's status as one word, folded: `status_key` in the rule engine. Exported for the tests.
 */
export function foldStatus(value: string): string {
  return canonicalKey(value).replaceAll(' ', '');
}

/** How the values of a condition of this kind are told apart. */
export function keyOfKind(kind: string): (value: string) => string {
  if (kind === 'certification_in') return foldCertification;
  if (kind === 'status_is') return foldStatus;
  return canonicalKey;
}

/** Append unless an equivalent spelling is already there. */
export function addValue(values: string[], value: string, key = canonicalKey): string[] {
  const added = key(value);
  if (!canonicalKey(value) || values.some((existing) => key(existing) === added)) return values;
  return [...values, value.trim()];
}

/** Drop every spelling equivalent to `value`. */
export function removeValue(values: string[], value: string, key = canonicalKey): string[] {
  const removed = key(value);
  return values.filter((existing) => key(existing) !== removed);
}

/**
 * The same condition with each value once, as matching compares them. The
 * server refuses a repeat on a write, but a stored rule can still arrive with
 * one, and the editor keys each chip by its value.
 */
export function withoutRepeats(condition: Condition): Condition {
  const { value } = condition;
  if (!Array.isArray(value)) return condition;
  const key = keyOfKind(condition.type);
  const kept = value.every((item) => typeof item === 'string')
    ? value.reduce<string[]>((list, item: string) => addValue(list, item, key), [])
    : [...new Set(value)];
  return { ...condition, value: kept };
}

/**
 * Split a comma-separated input, dropping blanks and equivalent repeats, for the axes the library
 * cannot enumerate. A comma never means "and": every value is an alternative to the others.
 */
export function parseStringList(input: string): string[] {
  return input
    .split(',')
    .map((part) => part.trim())
    .filter(Boolean)
    .reduce<string[]>((kept, value) => addValue(kept, value), []);
}

/**
 * Digits only: `Number` alone reads `0x25B` and `1e3` as numbers and `6O4` as nothing, which drops
 * a title unseen. Past a safe integer the digits read back as `1e+24`, an id no source has.
 */
const isIdentifier = (part: string) =>
  /^\d+$/.test(part) && Number(part) > 0 && Number.isSafeInteger(Number(part));

/** Split a list of external identifiers: whole numbers above zero, once each. */
export function parseNumberList(input: string): number[] {
  const ids = input
    .split(',')
    .map((part) => part.trim())
    .filter(isIdentifier)
    .map(Number);
  return [...new Set(ids)];
}

/** What `parseNumberList` drops from a list as typed, for the field to name. */
export function rejectedNumbers(input: string): string[] {
  return input
    .split(',')
    .map((part) => part.trim())
    .filter((part) => part !== '' && !isIdentifier(part));
}

/** The highest priority a rule may take: `MAX_PRIORITY` in the rule engine, the authority. */
export const MAX_PRIORITY = 1_000_000;

/**
 * Where a new rule goes: after every other, as the server places one given no priority, held at
 * `MAX_PRIORITY` as the server holds it, since a priority past it is refused.
 */
export function nextPriority(priorities: number[]): number {
  return Math.min(Math.max(0, ...priorities) + 10, MAX_PRIORITY);
}

/**
 * `MIN_YEAR` and `MAX_YEARS_AHEAD` of the rule engine, which holds the bounds: these stop the
 * browser offering a year the server refuses.
 */
export const MIN_YEAR = 1888;
export const maxYear = (now = new Date()) => now.getFullYear() + 5;

/** The longest runtime a rule may name, in minutes: `MAX_RUNTIME` in the rule engine. */
export const MAX_RUNTIME = 1440;

/** A score bound as typed, `7.5` or `7,5`, or `null` for none and for what is not a number. */
export function parseScoreBound(input: string): number | null {
  const trimmed = input.trim().replace(',', '.');
  if (!/^\d+(\.\d+)?$/.test(trimmed)) return null;
  return Number(trimmed);
}

/**
 * A whole number, or null to clear the bound rather than send 0. `Number` accepts `2e5` and
 * ` 12 ` as finite, and either stored as a year or a runtime makes a rule that matches nothing.
 */
export function parseWholeBound(input: string): number | null {
  const trimmed = input.trim();
  if (!trimmed) return null;
  if (!/^-?\d+$/.test(trimmed)) return null;
  const year = Number(trimmed);
  // `-0` is the zero it reads as, and digits past an exact integer no year.
  return Number.isSafeInteger(year) ? year + 0 : null;
}

/**
 * Whether a condition can be offered on a rule of this media type. A `both` rule meets films and
 * series, so it takes the intersection: a condition only one carries never holds for the other.
 * Scoping the rule to that type is what expresses "series only".
 */
export function conditionAppliesTo(spec: ConditionSpec, mediaType: string): boolean {
  if (mediaType === 'both') {
    return ['movie', 'series'].every((type) => spec.media_types.includes(type));
  }
  return spec.media_types.includes(mediaType);
}

/**
 * Name what the library holds from the vocabulary that defines it, or the held codes are the only
 * ones shown bare (`en` above `Afrikaans (af)`) in the rule builder and the panel above the table.
 */
export function nameFacets(held: Facet[], vocabulary: Facet[]): Facet[] {
  if (!vocabulary.length) return held;
  const names = new Map(vocabulary.map((facet) => [canonicalKey(facet.value), facet.label]));
  return held.map((facet) => ({
    ...facet,
    label: facet.label ?? names.get(canonicalKey(facet.value)),
  }));
}

/**
 * What a condition on `axis` can hold, named: the library's values, then the rest of the closed
 * vocabulary. The editor offers these and the rule table names stored values from them, so one
 * value never has two names. Hand in `localFacets` for names in the reader's language.
 */
export function facetOf(facets: LibraryFacets | null, axis: string | undefined): Facet[] {
  if (!axis || !facets) return [];
  // `every_axis_a_spec_names_is_one_the_facets_payload_answers` holds `axis` to these keys.
  const held = facets[axis as FacetAxis] ?? [];
  // Only two axes have a closed vocabulary, offered whole so an unsynced code is never guessed.
  const vocabulary = facets.vocabularies[axis as keyof Vocabularies] ?? [];
  if (!vocabulary.length) return held;

  const seen = new Set(held.map((facet) => canonicalKey(facet.value)));
  return [
    ...nameFacets(held, vocabulary),
    ...vocabulary.filter((facet) => !seen.has(canonicalKey(facet.value))),
  ];
}

/**
 * The library's facets with languages and regions named in the reader's language, counts and
 * vocabularies alike. Every screen naming a value reads them through this, so a language never
 * reads in English on one screen and in the reader's language on another.
 */
export function localFacets(facets: LibraryFacets, language: string): LibraryFacets {
  const rename = (type: 'language' | 'region', list: Facet[] = []) =>
    list.map((facet) => ({
      ...facet,
      label: localName(type, facet.value, language) ?? facet.label,
    }));
  return {
    ...facets,
    original_languages: rename('language', facets.original_languages),
    origin_countries: rename('region', facets.origin_countries),
    vocabularies: {
      ...facets.vocabularies,
      original_languages: rename('language', facets.vocabularies.original_languages),
      origin_countries: rename('region', facets.vocabularies.origin_countries),
    },
  };
}

const ISSUE_FIELDS: Record<string, string> = {
  name: 'rules-rule-name',
  description: 'rules-description',
  target_category: 'rules-target-category',
  media_type: 'rules-applies-to',
  priority: 'rules-priority',
};

/**
 * The id of the rule editor's control `issue` is about: a condition's row when it names the
 * condition's place, from 1, the list's add picker when it names the list alone. Null for a field
 * the editor does not draw.
 */
export function issueField(issue: Pick<ValidationIssue, 'field' | 'params'>): string | null {
  if (issue.field === 'conditions' || issue.field === 'exclusions') {
    const place = Number(issue.params.index);
    return Number.isInteger(place) && place > 0
      ? `rules-${issue.field}-${place - 1}`
      : `rules-${issue.field}-add`;
  }
  return ISSUE_FIELDS[issue.field] ?? null;
}
