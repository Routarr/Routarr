// Coercion between the rule editor's text inputs and the JSON the backend
// expects. Getting this wrong silently stores a rule that can never match, so
// it lives here as pure functions rather than inline in the form.

import type { ConditionSpec, Facet } from './types';

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
 * The form two spellings of one value share, mirroring `normalise_value` in the
 * rule engine.
 *
 * Only used to tell values apart in the interface — to keep a chip from being
 * added twice under `Science-Fiction` and `Science Fiction`, and to filter the
 * list without demanding the exact case or accent. What is stored and sent is
 * always the value as the library spells it.
 */
export function canonicalKey(value: string): string {
  return value
    .normalize('NFD')
    .replace(/\p{Diacritic}/gu, '')
    .toLowerCase()
    .replace(/[^\p{L}\p{N}]+/gu, ' ')
    .trim();
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
 * Split a comma-separated input, dropping blanks and equivalent repeats.
 *
 * The free-text path, kept for the axes the library cannot enumerate — a
 * keyword, a title fragment. A comma separates values and never means "and":
 * every value in one condition is an alternative to the others.
 */
export function parseStringList(input: string): string[] {
  return input
    .split(',')
    .map((part) => part.trim())
    .filter(Boolean)
    .reduce<string[]>((kept, value) => addValue(kept, value), []);
}

/** Same, for external identifiers. Non-numeric fragments are discarded. */
export function parseNumberList(input: string): number[] {
  return input
    .split(',')
    .map((part) => Number(part.trim()))
    .filter((value) => Number.isFinite(value) && value !== 0);
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
 * condition that only one of them carries can never hold for the other — the
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
 * in both needs both. Read from the counts alone, the five language codes
 * actually synced were the only ones shown bare — `en` and `fr` above a list of
 * `Afrikaans (af)`, which reads as the known ones being the ones nobody
 * bothered to name.
 *
 * Stated here rather than at each call site: two screens read these axes, the
 * rule builder's picker and the panel above the rule table, and only one of
 * them had it.
 */
export function nameFacets(held: Facet[], vocabulary: Facet[]): Facet[] {
  if (!vocabulary.length) return held;
  const names = new Map(vocabulary.map((facet) => [canonicalKey(facet.value), facet.label]));
  return held.map((facet) => ({
    ...facet,
    label: facet.label ?? names.get(canonicalKey(facet.value)),
  }));
}
