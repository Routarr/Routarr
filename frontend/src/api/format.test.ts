import { describe, expect, it } from 'vitest';
import { describeCondition, formatBytes, formatPercent, formatTimestamp } from './format';

describe('formatBytes', () => {
  const plain = (value: string) => value.replace(/\u202f|\u00a0/g, ' ');

  it('scales in binary steps and rounds to one decimal past bytes', () => {
    expect(plain(formatBytes(2048))).toBe('2.0 kB');
    expect(plain(formatBytes(5_368_709_120))).toBe('5.0 GB');
  });

  /**
   * The short form spaces the unit the way the language does; the narrow form
   * holds the abbreviation. Short alone writes the byte unit as `byte` in
   * English and `Byte` in German — a wart on the one value that reaches it,
   * an exact zero — and narrow alone drops the space French puts before it.
   */
  it("abbreviates the byte unit without losing the language's spacing", () => {
    expect(plain(formatBytes(0))).toBe('0 B');
    expect(plain(formatBytes(512))).toBe('512 B');
    expect(plain(formatBytes(512, 'de'))).toBe('512 B');
    expect(plain(formatBytes(512, 'fr'))).toBe('512 o');
  });

  /**
   * The byte symbol, where `Intl` has no abbreviation for it.
   *
   * The narrow form was taken to be the abbreviation, and for most of the
   * shipped set it is — but Dutch, Greek, Turkish, Korean and Traditional
   * Chinese answer with a *word*, so every size under 1 KiB read `512 byte` in
   * five languages. The symbol is derived from the kilobyte's instead, which is
   * the same symbol with an SI prefix in front of it, and that agrees with
   * `Intl` everywhere it does abbreviate.
   */
  it('abbreviates the byte unit even where Intl spells it out', () => {
    expect(plain(formatBytes(512, 'nl'))).toBe('512 B');
    expect(plain(formatBytes(512, 'el'))).toBe('512 B');
    expect(plain(formatBytes(512, 'tr'))).toBe('512 B');
    expect(plain(formatBytes(512, 'zh_TW'))).toBe('512 B');
    // Korean writes no space before a unit, which is Korean and not a fault.
    expect(plain(formatBytes(512, 'ko'))).toBe('512B');
    // And the languages that do abbreviate are untouched.
    expect(plain(formatBytes(512, 'ar'))).toBe('512 \u0628');
    expect(plain(formatBytes(512, 'ru'))).toBe('512 \u0411');
    expect(plain(formatBytes(512, 'fi'))).toBe('512 t');
  });

  it('stops at the largest unit it knows', () => {
    expect(formatBytes(1024 ** 6)).toContain('PB');
  });

  /**
   * The unit follows `ui_language`, not the source. French writes `o`, `ko`,
   * `Go`; every screen showing a size said `B`, `KB`, `GB` whatever the
   * interface language was. `Intl` knows the whole shipped set, so a language
   * added later is right without a translation of its own.
   */
  it("writes the unit in the reader's language", () => {
    expect(plain(formatBytes(5_368_709_120, 'fr'))).toBe('5,0 Go');
    expect(plain(formatBytes(2048, 'fr'))).toBe('2,0 ko');
    expect(plain(formatBytes(512, 'fr'))).toBe('512 o');
    expect(plain(formatBytes(5_368_709_120, 'ru'))).toBe('5,0 ГБ');
    expect(plain(formatBytes(5_368_709_120, 'de'))).toBe('5,0 GB');
  });

  /// The settings store `zh_CN`; BCP-47 wants a hyphen, and an unfixed
  /// underscore makes `Intl` throw rather than fall back.
  it('accepts the stored locale form', () => {
    expect(formatBytes(5_368_709_120, 'zh_CN')).toContain('5.0');
  });

  it('says nothing rather than zero when the Arr reported no size', () => {
    expect(formatBytes(null)).toBe('-');
    expect(formatBytes(undefined)).toBe('-');
    expect(formatBytes(-1)).toBe('-');
    expect(plain(formatBytes(0, 'fr'))).toBe('0 o');
    expect(plain(formatBytes(0))).toBe('0 B');
  });
});

describe('describeCondition', () => {
  it('joins list values', () => {
    expect(describeCondition({ type: 'genre_contains', value: ['Animation', 'Family'] })).toBe(
      'genre_contains: Animation, Family',
    );
  });

  it('marks an empty list, which can never match', () => {
    expect(describeCondition({ type: 'genre_contains', value: [] })).toBe(
      'genre_contains: (empty)',
    );
  });

  it('renders open-ended year ranges', () => {
    expect(describeCondition({ type: 'year_range', value: { min: 1980, max: null } })).toBe(
      'year_range: 1980 → *',
    );
    expect(describeCondition({ type: 'year_range', value: { min: null, max: null } })).toBe(
      'year_range: * → *',
    );
  });

  it('renders scalars', () => {
    expect(describeCondition({ type: 'has_files', value: true })).toBe('has_files: true');
    expect(describeCondition({ type: 'added_within_days', value: 7 })).toBe('added_within_days: 7');
  });

  it('tolerates a missing value', () => {
    expect(describeCondition({ type: 'has_files' })).toBe('has_files');
  });
});

describe('formatTimestamp', () => {
  it('renders a stored UTC timestamp in the configured language', () => {
    // Same instant, two languages: month-first against day-first.
    const en = formatTimestamp('2026-08-22 14:05:57', 'en-US');
    const fr = formatTimestamp('2026-08-22 14:05:57', 'fr-FR');

    expect(en).toContain('8/22');
    expect(fr).toContain('22/08');
  });

  it('treats the stored value as UTC rather than local time', () => {
    // Without the Z, a browser east of Greenwich would shift the hour and a
    // sync could appear to have run in the future.
    const utc = formatTimestamp('2026-08-22 14:05:57', 'en-GB');
    const explicit = formatTimestamp('2026-08-22T14:05:57Z', 'en-GB');

    expect(utc).toBe(explicit);
  });

  it('drops the seconds', () => {
    expect(formatTimestamp('2026-08-22 14:05:57', 'en-GB')).not.toContain('57');
  });

  it('maps a Servarr language code onto a BCP-47 locale', () => {
    // nb_NO would throw as a locale; the underscore has to become a hyphen.
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
  /// French puts a non-breaking space before the sign and English does not,
  /// which is why this goes through `Intl` rather than a template string.
  it('writes the sign the way the language writes it', () => {
    expect(formatPercent(0.7, 'en')).toBe('70%');
    expect(formatPercent(0.7, 'fr').replace(/\u202f|\u00a0/g, ' ')).toBe('70 %');
  });

  /// The underscore form is what the settings store; BCP-47 wants a hyphen,
  /// and an unfixed `zh_CN` throws rather than falling back.
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
