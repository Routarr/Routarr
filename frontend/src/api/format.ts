import type { Condition } from './types';

/**
 * Human-readable size in the reader's language, or an em dash when the Arr
 * did not report one.
 *
 * `B`, `KB`, `GB` were written out in English on every screen that shows a
 * size, whatever `ui_language` said — French writes `o`, `Ko`, `Go`, and so on
 * down the shipped set. `Intl` knows every one of them, so this costs no
 * translation and stays right for a language added later.
 *
 * The scale stays binary, as it was: the figure is compared against what a
 * file manager reports and against the backend's own `human_bytes`, and
 * changing the arithmetic to match the decimal names would move every number
 * on screen. It is the same convention every desktop uses — a mebibyte called
 * a megabyte — and it is the label, not the value, that was wrong here.
 *
 * Two forms are combined, and each contributes what it gets right: `short`
 * spaces the unit the way the language does — French puts one before it,
 * Korean does not — while `narrow` holds the abbreviation. Short alone writes
 * the byte unit as `byte` in English and `Byte` in German; narrow alone drops
 * the French space. Neither token is hard-coded, so a language added later is
 * right without a translation of its own.
 *
 * The byte symbol is the exception, and it is derived rather than asked for:
 * `narrow` answers with a *word* in Dutch, Greek, Turkish, Korean and
 * Traditional Chinese, so every size under 1 KiB read `512 byte` in five of
 * the languages shipped. `byteSymbol` takes it off the kilobyte's instead —
 * the same symbol with an SI prefix in front — which agrees with `Intl`
 * everywhere it does abbreviate.
 */
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

/** A backend enum value, lower-case, as the head of a PascalCase dictionary key. */
export const capitalize = (value: string): string => value.charAt(0).toUpperCase() + value.slice(1);

export function formatBytes(bytes: number | null | undefined, language = 'en'): string {
  if (bytes === null || bytes === undefined) return '—';
  if (bytes < 0) return '—';

  const units = ['byte', 'kilobyte', 'megabyte', 'gigabyte', 'terabyte', 'petabyte'] as const;
  let value = bytes;
  let unit = 0;
  while (value >= 1024 && unit < units.length - 1) {
    value /= 1024;
    unit += 1;
  }

  const digits = unit === 0 ? 0 : 1;
  try {
    const locale = language.replace('_', '-');
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

/** One-line summary of a condition, for the rules table. */
export function describeCondition(condition: Condition): string {
  const value = condition.value;

  if (Array.isArray(value)) {
    return value.length > 0
      ? `${condition.type}: ${value.join(', ')}`
      : `${condition.type}: (empty)`;
  }
  if (value && typeof value === 'object') {
    const range = value as { min?: number | null; max?: number | null };
    return `${condition.type}: ${range.min ?? '*'} → ${range.max ?? '*'}`;
  }
  if (value === undefined || value === null) return condition.type;
  return `${condition.type}: ${String(value)}`;
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
    return new Intl.NumberFormat(language.replace('_', '-'), {
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
 * monospace characters — wide enough to push a table cell onto a second line —
 * and it reads like a log file rather than a date.
 *
 * `Intl` does the formatting, so this costs no translation keys: the app
 * language is the locale. Language codes are stored Servarr-style (`nb_NO`,
 * `zh_CN`); BCP-47 wants a hyphen. Seconds are dropped — nobody schedules a
 * sync to the second — and the value is returned unchanged if it cannot be
 * parsed, since a visibly odd string beats a silent "Invalid Date".
 */
export function formatTimestamp(
  value: string | null | undefined,
  language: string,
  fallback = '—',
): string {
  if (!value) return fallback;

  // The stored form has no zone marker; it is UTC, and saying so avoids the
  // browser reading it as local time and shifting it by the offset.
  const parsed = new Date(value.includes('T') ? value : `${value.replace(' ', 'T')}Z`);
  if (Number.isNaN(parsed.getTime())) return value;

  try {
    return new Intl.DateTimeFormat(language.replace('_', '-'), {
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
 * stops helping — "3 months ago" is vaguer than the date — so it hands back.
 *
 * `Intl.RelativeTimeFormat` does the wording, so this costs no translation
 * keys, exactly as `formatTimestamp` does for dates.
 */
export function formatRelative(
  value: string | null | undefined,
  language: string,
  fallback = '—',
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
    const format = new Intl.RelativeTimeFormat(language.replace('_', '-'), { numeric: 'auto' });
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
