import { api } from '../api/client';

type Dictionary = Record<string, string>;
type Params = Record<string, string | number>;

/**
 * The dictionary comes from the backend, already merged over English, so the
 * frontend ships no strings of its own and an untranslated key still renders
 * readable text. This mirrors how the other Servarr applications work.
 *
 * A module-level rune rather than a context: there is exactly one dictionary in
 * the application, it is fetched once, and every component wants the same one.
 * Threading a provider through the tree would model a choice nobody has.
 */
const state = $state({
  strings: {} as Dictionary,
  language: 'en',
  direction: 'ltr' as 'ltr' | 'rtl',
  ready: false,
});

function substitute(template: string, params?: Params): string {
  if (!params || !template.includes('{')) return template;
  return Object.entries(params).reduce(
    (out, [name, value]) => out.split(`{${name}}`).join(String(value)),
    template,
  );
}

/**
 * Look a key up. Falls back to the key itself, which is the production
 * behaviour too: a dictionary that failed to load leaves an interface that is
 * ugly but navigable, rather than blank.
 */
export function t(key: string, params?: Params): string {
  return substitute(state.strings[key] ?? key, params);
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

/** Reload after the language setting changed. */
export async function loadDictionary(): Promise<void> {
  try {
    const response = await api.getLocalization();
    state.strings = response.strings;
    state.language = response.language;
    document.documentElement.lang = response.language;
    // The backend owns the script list, so adding an RTL language there turns
    // the interface around with nothing to change here.
    state.direction = response.direction ?? 'ltr';
    document.documentElement.dir = state.direction;
  } catch {
    // An unreachable backend must not blank the interface: keys render as
    // themselves, which is ugly but navigable.
  } finally {
    state.ready = true;
  }
}

/** Seed the dictionary directly. Tests only — nothing else should write it. */
export function seedDictionary(strings: Dictionary, language = 'en'): void {
  state.strings = strings;
  state.language = language;
  state.ready = true;
}

/**
 * Put the chosen theme on the root element.
 *
 * `auto` sets no attribute at all, which is what lets the
 * `prefers-color-scheme` block in the stylesheet apply. An explicit choice
 * stamps the attribute and outranks it — the same three-state arrangement the
 * showcase site uses.
 */
export function applyTheme(theme: string | undefined): void {
  if (theme === 'dark' || theme === 'light') document.documentElement.dataset.theme = theme;
  else delete document.documentElement.dataset.theme;
}
