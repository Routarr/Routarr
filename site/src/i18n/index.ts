import en from './en.json';
import fr from './fr.json';
import de from './de.json';
import es from './es.json';

export { LANGUAGES, type Locale } from './languages';
import { LANGUAGES, type Locale } from './languages';

/** English is the shape every other file is checked against. */
export type Key = keyof typeof en;

const CATALOGUE: Record<Locale, Record<string, string>> = { en, fr, de, es };

/**
 * Look a string up, and refuse to build without it.
 *
 * The markup names a key and the catalogue answers, so a copy edit to the
 * English text is just a copy edit. Translating by splicing a translation over
 * each English fragment of the finished HTML would put the whole burden on
 * exact string matching, where a sentence moved by a copy edit invalidates the
 * map and the only safety net is counting occurrences.
 *
 * What stands in for that count is this
 * throw: a key that no locale defines fails the build, and — because `Key` is
 * derived from `en.json` — a key that English does not have fails the type
 * check before that. A missing translation never renders as an empty element.
 */
export function useTranslations(locale: Locale) {
  const dictionary = CATALOGUE[locale];
  const english = CATALOGUE.en;

  return function t(key: Key): string {
    const value = dictionary[key] ?? english[key];
    if (value === undefined) {
      throw new Error(`i18n: no string for "${key}" in ${locale} or English`);
    }
    return value;
  };
}


/** Where a given language's copy of this page lives. */
export function pathFor(locale: Locale): string {
  return LANGUAGES.find((l) => l.code === locale)!.path;
}
