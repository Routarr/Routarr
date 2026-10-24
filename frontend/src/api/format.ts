import type { Condition, DecisionAction, LogAction, MediaType } from './types';

/**
 * The byte symbol off the kilobyte's: what follows the last `.`, as in Arabic's `\u0643.\u0628`,
 * else the first character, as in `ko`. Nothing when the kilobyte is itself a word, so the
 * caller keeps what `Intl` gave rather than truncating prose.
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
 * The language setting as `Intl` and the page's `lang` read it. `Intl` refuses the underscore of
 * `nb_NO` or `zh_TW`, so each formatter would throw, and no browser picks a font from it.
 */
export const bcp47 = (language: string) => language.replace('_', '-');

/**
 * An address without the `user:pass@` before its host, which a browser opening it would offer to
 * sign in with. The server answers them masked.
 */
export const withoutCredentials = (address: string): string =>
  address.replace(/^([a-z][a-z0-9+.-]*:\/\/)[^/?#]*@/i, '$1');

/** Names a home network gives a host, which no public DNS answers. */
const LOCAL_SUFFIXES = ['.local', '.lan', '.home', '.internal', '.localdomain', '.home.arpa'];

/**
 * Whether an address sends the Arr's key in clear past the home network: `http` to a public IP
 * address, or to a name a public DNS could answer. A container name, `localhost`, a private or
 * link-local address and a home network's name stay home.
 */
export function inClearPastHome(address: string): boolean {
  let url: URL;
  try {
    url = new URL(address.trim());
  } catch {
    return false;
  }
  if (url.protocol !== 'http:') return false;
  const host = url.hostname.toLowerCase().replace(/^\[|\]$/g, '');
  if (host.includes(':')) {
    return !(host === '::1' || /^f[cd]/.test(host) || /^fe[89ab]/.test(host));
  }
  const octets = host.split('.').map(Number);
  if (octets.length === 4 && octets.every((octet) => Number.isInteger(octet) && octet <= 255)) {
    const [a, b] = octets as [number, number, number, number];
    const home =
      a === 10 ||
      a === 127 ||
      (a === 172 && b >= 16 && b <= 31) ||
      (a === 192 && b === 168) ||
      (a === 169 && b === 254) ||
      (a === 100 && b >= 64 && b <= 127);
    return !home;
  }
  if (!host.includes('.') || host === 'localhost') return false;
  return !LOCAL_SUFFIXES.some((suffix) => host.endsWith(suffix));
}

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
  // A noun, beside `ActionMove`: `Revert` is the button's imperative.
  revert: 'ActionRevert',
};

/** The caption of each field a metadata source supplies, as the facets panel names it. */
export const METADATA_FIELD_KEY: Record<string, string> = {
  genres: 'FacetGenres',
  keywords: 'FacetKeywords',
  original_language: 'FacetLanguages',
  origin_countries: 'FacetCountries',
  certification: 'FacetCertifications',
  ratings: 'FacetRatings',
};

/** The dictionary key naming a kind of title. */
export const mediaTypeKey = (type: MediaType): string => (type === 'movie' ? 'Movies' : 'Series');

/**
 * A path placed in a sentence, in a first strong isolate: in a right-to-left message its leading
 * slash otherwise lands at the far end, and its segments read in the message's direction.
 */
export const isolated = (text: string): string => `\u2068${text}\u2069`;

/**
 * A size in the reader's language, on the binary scale of a file manager and of `human_bytes`.
 * `short` spaces the unit as the language does and `narrow` holds the abbreviation. The byte's,
 * a word in several languages, comes from `byteSymbol`.
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
    // A locale `Intl` refuses, or an engine without the unit style, still shows the figure.
    return `${value.toFixed(digits)} ${['B', 'KB', 'MB', 'GB', 'TB', 'PB'][unit]}`;
  }
}

/** The words a condition is described with, all of them the reader's language. */
interface ConditionWords {
  /** The condition's caption from the catalogue, or its type where it has none. */
  label?: string;
  /** A sentence of the dictionary, its placeholders filled in, as `t()` writes it. */
  phrase: (key: string, params?: Record<string, string>) => string;
  /** Joins list values, as `t('ListSeparator')` writes it. */
  separator: string;
  /** Said of a list with no value, which can never match. */
  empty: string;
  /** The name a stored value is shown under, where it has one. */
  name?: (value: string) => string;
  /** A score scale's name, `IMDb` for `imdb`. */
  scale?: (id: string) => string;
  /** A number as the language writes it, `7,5` in French. */
  number?: (value: number) => string;
}

/**
 * The dictionary key of a condition's whole sentence, its values in a placeholder, so each
 * language declines "any of" or "none of" as its grammar wants. `Not` is a yes or no condition
 * said no, `From` and `To` a year range open at one end.
 */
export const phraseKey = (type: string, form: '' | 'Not' | 'From' | 'To' = ''): string =>
  `ConditionPhrase${type.split('_').map(capitalize).join('')}${form}`;

/** One-line summary of a condition, for the rules table. */
export function describeCondition(condition: Condition, words: ConditionWords): string {
  const caption = words.label ?? condition.type;
  const key = (form: '' | 'Not' | 'From' | 'To' = '') => phraseKey(condition.type, form);
  const value = condition.value;

  if (Array.isArray(value)) {
    const named = value.map((each) => (words.name ? words.name(String(each)) : String(each)));
    const values = named.length > 0 ? named.join(words.separator) : words.empty;
    return words.phrase(key(), { values });
  }
  if (value && typeof value === 'object') {
    const { min, max, scale, min_votes } = value as {
      min?: number | null;
      max?: number | null;
      scale?: string;
      min_votes?: number | null;
    };
    // A score's fraction in the reader's form, a year or a runtime as written: `1 990` is no year.
    const shown = (n: number) => (scale && words.number ? words.number(n) : `${n}`);
    const named: Record<string, string> = scale ? { scale: words.scale?.(scale) ?? scale } : {};
    const range =
      min != null && max != null
        ? words.phrase(key(), { ...named, min: shown(min), max: shown(max) })
        : min != null
          ? words.phrase(key('From'), { ...named, min: shown(min) })
          : max != null
            ? words.phrase(key('To'), { ...named, max: shown(max) })
            : caption;
    if (min_votes == null) return range;
    const votes = words.phrase('ScoreVotesAtLeast', { votes: `${min_votes}` });
    return `${range}${words.separator}${votes}`;
  }
  if (typeof value === 'boolean') return words.phrase(key(value ? '' : 'Not'));
  if (value === undefined || value === null) return caption;
  return words.phrase(key(), { value: String(value) });
}

/** A number with its fraction as the language writes it, `7,5` in French, `8,0` with one digit. */
export function formatDecimal(value: number, language: string, minimumDigits = 0): string {
  try {
    return new Intl.NumberFormat(bcp47(language), {
      minimumFractionDigits: minimumDigits,
      maximumFractionDigits: 2,
    }).format(value);
  } catch {
    return String(value);
  }
}

/** A count with its digits grouped as the language groups them, `12 345` in French. */
export function formatCount(value: number, language: string): string {
  try {
    return new Intl.NumberFormat(bcp47(language), { maximumFractionDigits: 0 }).format(value);
  } catch {
    return String(value);
  }
}

/**
 * A 0 to 1 ratio as the language writes a percentage: French puts a non-breaking space before
 * the sign, and several locales use another sign.
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
 * A timestamp the server sends, RFC 3339 or as stored, UTC without a zone marker, which a
 * browser would read as local time. Invalid when it is neither.
 */
function parseStamp(value: string): Date {
  return new Date(value.includes('T') ? value : `${value.replace(' ', 'T')}Z`);
}

/**
 * A stored UTC timestamp as the configured language writes it, to the minute. One that cannot
 * be parsed comes back as it is, since a visibly odd string beats a silent "Invalid Date".
 */
export function formatTimestamp(
  value: string | null | undefined,
  language: string,
  fallback = '-',
): string {
  if (!value) return fallback;
  const parsed = parseStamp(value);
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
 * A moment of the last week as "4 minutes ago", an older one as its date: past a week the
 * relative form is vaguer than the date.
 */
export function formatRelative(
  value: string | null | undefined,
  language: string,
  fallback = '-',
): string {
  if (!value) return fallback;
  const parsed = parseStamp(value);
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
 * A language or a region named in the reader's language, the code in brackets, where the server's
 * vocabulary is spelled in English for every reader. `null` when the browser has no name for the
 * code, so the caller keeps the server's.
 */
export function localName(
  type: 'language' | 'region',
  code: string,
  language: string,
): string | null {
  try {
    const region = code.toUpperCase();
    // `Intl` names a retired code after its successor, `SU` as Russia: two Russias in a picker.
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

const DAY_MS = 86_400_000;

/** Whether a key expiring at `expiresAt` stops within the week the notification warns of. */
export function expiresWithinAWeek(expiresAt: string | null, now = Date.now()): boolean {
  if (!expiresAt) return false;
  const left = parseStamp(expiresAt).getTime() - now;
  return left > 0 && left <= 7 * DAY_MS;
}

/** Whether a key made at `createdAt` has gone 90 days without a call, `lastUsedAt` the last. */
export function idleFor90Days(
  lastUsedAt: string | null,
  createdAt: string,
  now = Date.now(),
): boolean {
  return now - parseStamp(lastUsedAt ?? createdAt).getTime() >= 90 * DAY_MS;
}

/** `list` with `value` added when a box is ticked, or taken out when it is cleared. */
export function toggled<T>(list: readonly T[], value: T, on: boolean): T[] {
  return on ? [...list, value] : list.filter((entry) => entry !== value);
}

/**
 * `items` with the entry at `index` traded for its neighbour `by` places away, or `null` with no
 * such neighbour. The list is left as it is: the screen redraws from the server's answer.
 */
export function swapped<T>(items: readonly T[], index: number, by: number): T[] | null {
  const target = index + by;
  const from = items[index];
  const to = items[target];
  // The bounds check, which also refuses a sparse array's hole that a length check lets through.
  if (from === undefined || to === undefined) return null;
  const next = [...items];
  next[index] = to;
  next[target] = from;
  return next;
}

/**
 * The entries in runs of one key, in order, each with its index in the whole list: a list drawn
 * in groups is still walked as one by the arrows.
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

/** The fields a person set, the empty ones left out: sent empty, a field narrows to nothing. */
export function setFields<T extends Record<string, string>>(fields: T): Partial<T> {
  return Object.fromEntries(
    Object.entries(fields).filter(([, value]) => value !== ''),
  ) as Partial<T>;
}

/**
 * The start of a `YYYY-MM-DD` day in the reader's time zone, as the instant a server reads,
 * and `undefined` for no day. `after` gives the start of the next one, the bound that keeps
 * the whole day in.
 */
export function dayStart(day: string, after = false): string | undefined {
  const [year, month, date] = day.split('-').map(Number);
  if (!year || !month || !date) return undefined;
  return new Date(year, month - 1, date + (after ? 1 : 0)).toISOString();
}
