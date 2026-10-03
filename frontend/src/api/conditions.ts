// Coercion between the rule editor's text inputs and the JSON the backend
// expects. Getting this wrong silently stores a rule that can never match, so
// it lives here as pure functions rather than inline in the form.

import type {
  Condition,
  ConditionSpec,
  Facet,
  FacetAxis,
  LibraryFacets,
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
      return { min: null, max: null };
    case 'string':
      return '';
    default:
      return [];
  }
}

/**
 * The Latin letters with a mark `fold_diacritic` reduces in the rule engine,
 * and no others: a general strip of marks would also join `ガ` with `カ` and
 * `ō` with `o`, which the engine keeps apart.
 */
const FOLDED: Record<string, string> = Object.fromEntries(
  (
    [
      ['àáâãäå', 'a'],
      ['ç', 'c'],
      ['èéêë', 'e'],
      ['ìíîï', 'i'],
      ['ñ', 'n'],
      ['òóôõöø', 'o'],
      ['ùúûü', 'u'],
      ['ýÿ', 'y'],
    ] as const
  ).flatMap(([marked, letter]) => [...marked].map((c) => [c, letter])),
);

/** What Rust's `char::is_alphanumeric` accepts. */
const ALPHANUMERIC = /[\p{Alphabetic}\p{N}]/u;

/**
 * The form two spellings of one value share: `normalise_value` in the rule
 * engine, character for character, both held to one table of cases.
 *
 * Only used to tell values apart in the interface: to keep a chip from being
 * added twice under `Science-Fiction` and `Science Fiction`, and to filter the
 * list without demanding the exact case or accent. What is stored and sent is
 * always the value as the library spells it.
 */
export function canonicalKey(value: string): string {
  let out = '';
  let gap = false;
  for (const c of value) {
    if (!ALPHANUMERIC.test(c)) {
      gap = true;
      continue;
    }
    if (gap && out) out += ' ';
    gap = false;
    for (const lower of c.toLowerCase()) out += FOLDED[lower] ?? lower;
  }
  return out;
}

/** Append unless an equivalent spelling is already there. */
export function addValue(values: string[], value: string): string[] {
  const key = canonicalKey(value);
  if (!key || values.some((existing) => canonicalKey(existing) === key)) return values;
  return [...values, value.trim()];
}

/** Drop every spelling equivalent to `value`. */
export function removeValue(values: string[], value: string): string[] {
  const key = canonicalKey(value);
  return values.filter((existing) => canonicalKey(existing) !== key);
}

/**
 * The same condition with each value once, as matching compares them. The
 * server refuses a repeat on a write, but a stored rule can still arrive with
 * one, and the editor keys each chip by its value.
 */
export function withoutRepeats(condition: Condition): Condition {
  const { value } = condition;
  if (!Array.isArray(value)) return condition;
  const kept = value.every((item) => typeof item === 'string')
    ? value.reduce<string[]>((list, item: string) => addValue(list, item), [])
    : [...new Set(value)];
  return { ...condition, value: kept };
}

/**
 * Split a comma-separated input, dropping blanks and equivalent repeats.
 *
 * The free-text path, kept for the axes the library cannot enumerate (a
 * keyword, a title fragment). A comma separates values and never means "and":
 * every value in one condition is an alternative to the others.
 */
export function parseStringList(input: string): string[] {
  return input
    .split(',')
    .map((part) => part.trim())
    .filter(Boolean)
    .reduce<string[]>((kept, value) => addValue(kept, value), []);
}

/** Same, for external identifiers: whole numbers above zero, once each. */
export function parseNumberList(input: string): number[] {
  const ids = input
    .split(',')
    .map((part) => Number(part.trim()))
    .filter((value) => Number.isInteger(value) && value > 0);
  return [...new Set(ids)];
}

/**
 * The first surviving film, and the furthest ahead a rule may reach. Mirrors
 * `MIN_YEAR` and `MAX_YEARS_AHEAD` in the rule engine, which is the authority:
 * these two only stop the browser offering a value the server will refuse.
 */
export const MIN_YEAR = 1888;
export const maxYear = (now = new Date()) => now.getFullYear() + 5;

/**
 * An empty input clears the bound rather than sending 0.
 *
 * A whole number or nothing: `Number` accepts `2e5` and ` 12 ` as finite, and
 * turning either into a year silently stores a rule that matches nothing.
 */
export function parseYearBound(input: string): number | null {
  const trimmed = input.trim();
  if (!trimmed) return null;
  if (!/^-?\d+$/.test(trimmed)) return null;
  return Number(trimmed);
}

/**
 * Whether a condition can be offered on a rule of this media type.
 *
 * A rule scoped to `both` is evaluated against films *and* series, so a
 * condition that only one of them carries can never hold for the other. The
 * answer there is the intersection, not the union. Scoping the rule to the
 * type the condition needs is what expresses "series only".
 */
export function conditionAppliesTo(spec: ConditionSpec, mediaType: string): boolean {
  if (mediaType === 'both') {
    return ['movie', 'series'].every((type) => spec.media_types.includes(type));
  }
  return spec.media_types.includes(mediaType);
}

/**
 * Name what the library holds from the vocabulary that defines it.
 *
 * The library counts values and the closed vocabulary names them, and a value
 * in both needs both. Read from the counts alone, the codes the library holds
 * would be the only ones shown bare, `en` and `fr` above a list of
 * `Afrikaans (af)`, which reads as the known ones being the ones nobody
 * bothered to name.
 *
 * Stated here rather than at each call site: two screens read these axes, the
 * rule builder's picker and the panel above the rule table, and a copy in each
 * would let one of them show the codes bare.
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
 * The values a condition on `axis` can hold, each with its name: what the
 * library carries, named from the closed vocabulary, then the rest of that
 * vocabulary. The rule editor offers these, and the rule table names a stored
 * value from them, so the two never call one value by two names.
 *
 * `facets` are read as they come, so a caller naming values in the reader's
 * language hands in `localFacets`. The catalogue names the axis, and this reads
 * it off the payload, so a condition added in Rust needs no change here.
 */
export function facetOf(facets: LibraryFacets | null, axis: string | undefined): Facet[] {
  if (!axis || !facets) return [];
  // Narrowed, not widened: the name arrives from the backend so this is an
  // assertion either way, but `Record<string, Facet[]>` would erase every
  // later check as well. A test vouches for the name itself.
  const held = facets[axis as FacetAxis] ?? [];
  // A closed vocabulary is offered whole, the library's own values first so
  // the common answer stays at the top. Without it a language rule offers only
  // the codes that happen to be synced, and every other one has to be guessed,
  // as a code, which nobody would.
  // Only two axes have a closed vocabulary, so this indexing is partial by
  // design and the key may legitimately miss.
  const vocabulary = facets.vocabularies[axis as keyof Vocabularies] ?? [];
  if (!vocabulary.length) return held;

  const seen = new Set(held.map((facet) => canonicalKey(facet.value)));
  return [
    ...nameFacets(held, vocabulary),
    ...vocabulary.filter((facet) => !seen.has(canonicalKey(facet.value))),
  ];
}

/**
 * The library's facets with its languages and regions named in the reader's
 * language (`localName`), in the counts and in the closed vocabularies alike.
 * Every screen naming a value reads them through this, so a language never
 * reads in English on one and in the reader's language on another.
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
