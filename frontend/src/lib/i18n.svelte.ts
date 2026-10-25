import { api } from '../api/client';
import { bcp47, formatCount, isolated } from '../api/format';
import type { Localization } from '../api/types';

type Dictionary = Record<string, string>;
type Params = Record<string, string | number>;

/**
 * The dictionary comes from the backend, already merged over English, so an untranslated key
 * still reads. A module-level rune, as there is one dictionary for the whole interface.
 */
const state = $state({
  strings: {} as Dictionary,
  language: 'en',
  direction: 'ltr' as 'ltr' | 'rtl',
  /**
   * The placeholders that hold a count, grouped as the language groups digits (`12 345`). Only
   * these: a year in `{min}` grouped would read `2,026`. The server's list is `COUNTS` in
   * `backend/src/localization.rs`.
   */
  counts: [] as readonly string[],
});

function shown(name: string, value: Params[string] | undefined, isolating: boolean): string {
  const count = typeof value === 'number' && state.counts.includes(name);
  const text = count ? formatCount(value, state.language) : String(value);
  return isolating && !count ? isolated(text) : text;
}

// One pass over the template: a value is never read again, so a `{name}`
// inside it stays as written.
function substitute(template: string, params: Params | undefined, isolating: boolean): string {
  if (!params || !template.includes('{')) return template;
  return template.replace(/\{(\w+)\}/g, (whole, name: string) =>
    Object.hasOwn(params, name) ? shown(name, params[name], isolating) : whole,
  );
}

/**
 * Look a key up, falling back to the key, so a dictionary that failed to load leaves a
 * navigable interface. It substitutes `{placeholder}` from `params` and knows no plural.
 */
export function t(key: string, params?: Params): string {
  // Right to left, every value but a count is isolated, as the server does, or a path reorders.
  return substitute(state.strings[key] ?? key, params, state.direction === 'rtl');
}

/**
 * A text the server keeps, as a pinned test's name: substituted as `t()` does, with no isolate,
 * since an invisible mark would make two names of one.
 */
export function stored(key: string, params?: Params): string {
  return substitute(state.strings[key] ?? key, params, false);
}

/**
 * The sentence `key` cut where each placeholder of `gaps` goes, so a value can be drawn there in
 * markup of its own. The pieces follow the order the translation writes the gaps in.
 */
export function cut(key: string, gaps: readonly string[], params: Params = {}): string[] {
  const holes = Object.fromEntries(gaps.map((gap) => [gap, '\u0000']));
  return t(key, { ...params, ...holes }).split('\u0000');
}

/** The current language, for `Intl` formatting. Reactive. */
export const i18n = {
  get language() {
    return state.language;
  },
  get direction() {
    return state.direction;
  },
};

function use(dictionary: Localization): void {
  state.strings = dictionary.strings;
  state.language = dictionary.language;
  state.counts = dictionary.counts;
  document.documentElement.lang = bcp47(dictionary.language);
  // The backend owns the script list, so adding an RTL language there turns
  // the interface around with nothing to change here.
  state.direction = dictionary.direction;
  document.documentElement.dir = state.direction;
  applyTheme(dictionary.theme);
}

/**
 * Before the first render: the strings the page was served with, in the
 * language set, which need no request, so a backend slow to answer or failing
 * never leaves a blank page followed by raw keys. The dev server serves the
 * page as built, with none, and they are fetched.
 */
export async function startDictionary(): Promise<void> {
  const carried = document.getElementById('dictionary')?.textContent;
  if (carried) {
    use(JSON.parse(carried) as Localization);
    return;
  }
  await loadDictionary();
}

/** Reload after the language setting changed. */
export async function loadDictionary(): Promise<void> {
  try {
    use(await api.getLocalization());
  } catch {
    // Unreachable, the keys render as themselves.
  }
}

/** Seed the dictionary directly. Tests only: nothing else may write it. */
export function seedDictionary(
  strings: Dictionary,
  language = 'en',
  counts: readonly string[] = [],
  direction: 'ltr' | 'rtl' = 'ltr',
): void {
  state.strings = strings;
  state.language = language;
  state.counts = counts;
  state.direction = direction;
}

/**
 * Put the chosen theme on the root element. `auto` sets no attribute, which lets the
 * stylesheet's `prefers-color-scheme` block apply.
 */
export function applyTheme(theme: string | undefined): void {
  if (theme === 'dark' || theme === 'light') document.documentElement.dataset.theme = theme;
  else delete document.documentElement.dataset.theme;
}
