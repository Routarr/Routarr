import type { Condition, DecisionAction, LogAction, MediaType } from './types';

/**
 * The byte symbol for a locale, off the kilobyte's.
 *
 * `ko` → `o`, `\u043a\u0411` → `\u0411`, `kt` → `t`, and Arabic's `\u0643.\u0628` → `\u0628`: an SI
 * prefix, then the symbol. Everything after the last `.` where there is one,
 * since Arabic separates the two, and everything after the first character
 * otherwise. Returns nothing when the kilobyte form is itself a word, so the
 * caller keeps whatever `Intl` gave it rather than truncating prose.
 */
function byteSymbol(locale: string): string | undefined {
  const kilo = new Intl.NumberFormat(locale, {
    style: 'unit',
    unit: 'kilobyte',
    unitDisplay: 'narrow',
  })
    .formatToParts(1)
    .find((part) => part.type === 'unit')?.value;
  if (kilo === undefined) return undefined;
  const symbol = kilo.includes('.') ? kilo.slice(kilo.lastIndexOf('.') + 1) : kilo.slice(1);
  return symbol.length > 0 && symbol.length <= 2 ? symbol : undefined;
}

/**
 * The language setting as `Intl` and the page's `lang` read it. The
 * dictionaries are named `nb_NO` or `zh_TW`, and `Intl` refuses the
 * underscore: each formatter would throw, and fall back to the server's
 * English. Neither is a language tag a browser picks a font or a voice from.
 */
export const bcp47 = (language: string) => language.replace('_', '-');

/**
 * An address without the `user:pass@` before its host, which a browser
 * opening it would offer to sign in with. The server answers them masked.
 */
export const withoutCredentials = (address: string): string =>
  address.replace(/^([a-z][a-z0-9+.-]*:\/\/)[^/?#]*@/i, '$1');

/** A backend enum value, lower-case, as the head of a PascalCase dictionary key. */
export const capitalize = (value: string): string => value.charAt(0).toUpperCase() + value.slice(1);

/** The dictionary key naming what set a job or a decision off. */
export const triggerKey = (trigger: string): string => `Trigger${capitalize(trigger)}`;

/** The dictionary key naming the status of a job or a decision. */
export const statusKey = (status: string): string => `Status${capitalize(status)}`;

/** The dictionary key naming what a decision proposes. */
export const DECISION_ACTION_KEY: Record<DecisionAction, string> = {
  move: 'ActionMove',
  none: 'ActionNone',
  skip: 'ActionSkip',
};

/** The dictionary key naming what a log entry wrote to an Arr. */
export const LOG_ACTION_KEY: Record<LogAction, string> = {
  move: 'ActionMove',
  revert: 'Revert',
};

/** The caption of each field a metadata source supplies, as the facets panel names it. */
export const METADATA_FIELD_KEY: Record<string, string> = {
  genres: 'FacetGenres',
  keywords: 'FacetKeywords',
  original_language: 'FacetLanguages',
  origin_countries: 'FacetCountries',
  certification: 'FacetCertifications',
};

/** The dictionary key naming a kind of title. */
export const mediaTypeKey = (type: MediaType): string => (type === 'movie' ? 'Movies' : 'Series');

/**
 * What a failed sync or probe ran into. The backend writes `error: ` and the
 * error's own text, which comes from the network or the Arr: a badge says
 * that it failed in the reader's language, and its title holds this.
 */
export const failureDetail = (status: string): string => status.replace(/^error: /, '');

/**
 * A path placed in a sentence, in a first strong isolate: in a right-to-left
 * message its leading slash otherwise lands at the far end, and its segments
 * read in the message's direction.
 */
export const isolated = (text: string): string => `\u2068${text}\u2069`;

/**
 * Human-readable size in the reader's language, or a hyphen when the Arr did
 * not report one.
 *
 * The unit names come from `Intl`, which knows each language's own (French
 * writes `o`, `Ko`, `Go`), so this costs no translation and stays right for a
 * language added later.
 *
 * The scale is binary: the figure is compared against what a file manager
 * reports and against the backend's own `human_bytes`. It is the convention
 * every desktop uses, a mebibyte called a megabyte.
 *
 * Two forms are combined, and each contributes what it gets right: `short`
 * spaces the unit the way the language does (French puts one before it,
 * Korean does not), while `narrow` holds the abbreviation. Short alone writes
 * the byte unit as `byte` in English and `Byte` in German, and narrow alone
 * drops the French space.
 *
 * The byte symbol is the exception, and it is derived rather than asked for:
 * `narrow` answers with a *word* in Dutch, Greek, Turkish, Korean and
 * Traditional Chinese. `byteSymbol` takes it off the kilobyte's instead, the
 * same symbol with an SI prefix in front, which agrees with `Intl` everywhere
 * it does abbreviate.
 */
export function formatBytes(bytes: number | null | undefined, language = 'en'): string {
  if (bytes === null || bytes === undefined) return '-';
  if (bytes < 0) return '-';

  const units = ['byte', 'kilobyte', 'megabyte', 'gigabyte', 'terabyte', 'petabyte'] as const;
  let value = bytes;
  let unit = 0;
  while (value >= 1024 && unit < units.length - 1) {
    value /= 1024;
    unit += 1;
  }

  const digits = unit === 0 ? 0 : 1;
  try {
    const locale = bcp47(language);
    const options = {
      style: 'unit',
      unit: units[unit],
      minimumFractionDigits: digits,
      maximumFractionDigits: digits,
    } as const;

    const abbreviation =
      (unit === 0 ? byteSymbol(locale) : undefined) ??
      new Intl.NumberFormat(locale, { ...options, unitDisplay: 'narrow' })
        .formatToParts(value)
        .find((part) => part.type === 'unit')?.value;

    return new Intl.NumberFormat(locale, { ...options, unitDisplay: 'short' })
      .formatToParts(value)
      .map((part) => (part.type === 'unit' ? (abbreviation ?? part.value) : part.value))
      .join('');
  } catch {
    // A locale `Intl` refuses, or an engine without the unit style: the figure
    // still has to reach the screen.
    return `${value.toFixed(digits)} ${['B', 'KB', 'MB', 'GB', 'TB', 'PB'][unit]}`;
  }
}

/** The words a condition is described with, all of them the reader's language. */
export interface ConditionWords {
  /** The condition's caption from the catalogue, or its type where it has none. */
  label?: string;
  /** Joins list values, as `t('ListSeparator')` writes it. */
  separator: string;
  /** Said of a list with no value, which can never match. */
  empty: string;
  /** A caption and its values, as `t('ConditionSummary')` writes them. */
  summary: (caption: string, values: string) => string;
  /** A range, as `t('ConditionRange')` writes it. */
  range: (min: string, max: string) => string;
  /** An open bound of a range. */
  open: string;
  /** The name a stored value is shown under, where it has one. */
  name?: (value: string) => string;
}

/** One-line summary of a condition, for the rules table. */
export function describeCondition(condition: Condition, words: ConditionWords): string {
  const caption = words.label ?? condition.type;
  const value = condition.value;

  if (Array.isArray(value)) {
    const named = value.map((each) => (words.name ? words.name(String(each)) : String(each)));
    return words.summary(caption, named.length > 0 ? named.join(words.separator) : words.empty);
  }
  if (value && typeof value === 'object') {
    const range = value as { min?: number | null; max?: number | null };
    const bound = (year: number | null | undefined) => (year == null ? words.open : String(year));
    return words.summary(caption, words.range(bound(range.min), bound(range.max)));
  }
  if (value === undefined || value === null) return caption;
  return words.summary(caption, String(value));
}

/**
 * A 0–1 ratio as the language writes a percentage.
 *
 * `Intl` rather than `${n}%`: French puts a non-breaking space before the
 * sign, English does not, and several locales use a different sign entirely.
 * Same underscore-to-hyphen fix as the date formats, for the same reason.
 */
export function formatPercent(value: number | null | undefined, language: string): string {
  const ratio = Math.min(1, Math.max(0, value ?? 0));
  try {
    return new Intl.NumberFormat(bcp47(language), {
      style: 'percent',
      maximumFractionDigits: 0,
    }).format(ratio);
  } catch {
    return `${Math.round(ratio * 100)}%`;
  }
}

/**
 * A timestamp the way the configured language writes it.
 *
 * The backend stores `YYYY-MM-DD HH:MM:SS` in UTC. Rendered raw it is nineteen
 * monospace characters, wide enough to push a table cell onto a second line,
 * and it reads like a log file rather than a date.
 *
 * `Intl` does the formatting, so this costs no translation keys: the app
 * language is the locale. Language codes are stored Servarr-style (`nb_NO`,
 * `zh_CN`), where BCP-47 wants a hyphen. Seconds are dropped: nobody
 * schedules a sync to the second. The value is returned unchanged if it
 * cannot be parsed, since a visibly odd string beats a silent "Invalid Date".
 */
export function formatTimestamp(
  value: string | null | undefined,
  language: string,
  fallback = '-',
): string {
  if (!value) return fallback;

  // The stored form has no zone marker. It is UTC, and saying so keeps the
  // browser from reading it as local time and shifting it by the offset.
  const parsed = new Date(value.includes('T') ? value : `${value.replace(' ', 'T')}Z`);
  if (Number.isNaN(parsed.getTime())) return value;

  try {
    return new Intl.DateTimeFormat(bcp47(language), {
      dateStyle: 'short',
      timeStyle: 'short',
    }).format(parsed);
  } catch {
    // An unknown locale must not blank the column.
    return parsed.toISOString().slice(0, 16).replace('T', ' ');
  }
}

/**
 * A recent moment as "4 minutes ago", anything older as its date.
 *
 * A dashboard and a log are read to answer "is this fresh?", and an absolute
 * timestamp makes the reader do the subtraction. Past a week the relative form
 * stops helping ("3 months ago" is vaguer than the date), so it hands back.
 *
 * `Intl.RelativeTimeFormat` does the wording, so this costs no translation
 * keys, exactly as `formatTimestamp` does for dates.
 */
export function formatRelative(
  value: string | null | undefined,
  language: string,
  fallback = '-',
): string {
  if (!value) return fallback;
  const parsed = new Date(value.includes('T') ? value : `${value.replace(' ', 'T')}Z`);
  if (Number.isNaN(parsed.getTime())) return value;

  const seconds = (parsed.getTime() - Date.now()) / 1000;
  const magnitude = Math.abs(seconds);
  if (magnitude > 7 * 86400) return formatTimestamp(value, language, fallback);

  const steps: [Intl.RelativeTimeFormatUnit, number][] = [
    ['second', 60],
    ['minute', 3600],
    ['hour', 86400],
    ['day', 7 * 86400],
  ];
  try {
    const format = new Intl.RelativeTimeFormat(bcp47(language), { numeric: 'auto' });
    let previous = 1;
    for (const [unit, limit] of steps) {
      if (magnitude < limit) return format.format(Math.round(seconds / previous), unit);
      previous = limit;
    }
    return format.format(Math.round(seconds / (7 * 86400)), 'week');
  } catch {
    // An unknown locale must not blank the column.
    return formatTimestamp(value, language, fallback);
  }
}

/**
 * A language or a region named in the reader's language, the code kept in
 * brackets as the server's vocabulary writes it. That vocabulary is spelled in
 * English for every reader, and the browser knows the names in all of them.
 * `null` when it has none for the code, so the caller keeps the server's.
 */
export function localName(
  type: 'language' | 'region',
  code: string,
  language: string,
): string | null {
  try {
    const region = code.toUpperCase();
    // A code that no longer exists is named after the country that replaced
    // it, `SU` as Russia and `DD` as Germany, so the picker would offer two
    // Russias and a rule on `RU` would miss every Soviet film. Left to the
    // caller, which names it from the server's table.
    if (type === 'region' && Intl.getCanonicalLocales(`und-${region}`)[0] !== `und-${region}`) {
      return null;
    }
    const names = new Intl.DisplayNames([bcp47(language)], { type, fallback: 'none' });
    const name = names.of(type === 'region' ? region : code);
    return name ? `${name} (${code})` : null;
  } catch {
    return null;
  }
}

/**
 * `items` with the entry at `index` traded for its neighbour `by` places away,
 * or `null` when there is no such neighbour. The list handed in is left as it
 * is: a reorder is sent to the server, and the screen redraws from its answer.
 */
export function swapped<T>(items: readonly T[], index: number, by: number): T[] | null {
  const target = index + by;
  const from = items[index];
  const to = items[target];
  // Reading both is the bounds check: past either end reads `undefined`, and so
  // does the hole of a sparse array, which a check on the length lets through.
  if (from === undefined || to === undefined) return null;
  const next = [...items];
  next[index] = to;
  next[target] = from;
  return next;
}

/**
 * The entries in runs of one key, in the order they came, each with its index
 * in the whole list. A list drawn in groups is still walked as one by the
 * arrows, and the index is the place they count.
 */
export function runsOf<T, K>(
  items: readonly T[],
  keyOf: (item: T) => K,
): { key: K; entries: { item: T; index: number }[] }[] {
  const runs: { key: K; entries: { item: T; index: number }[] }[] = [];
  items.forEach((item, index) => {
    const key = keyOf(item);
    const last = runs.at(-1);
    if (last && last.key === key) last.entries.push({ item, index });
    else runs.push({ key, entries: [{ item, index }] });
  });
  return runs;
}
