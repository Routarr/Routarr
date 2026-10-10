/**
 * Shipped languages, in the order the switcher shows them. English is first.
 *
 * The one list: `check.mjs` walks it to open every built page and
 * `astro.config.mjs` derives its locales from it, so a fifth language added
 * here is built, offered and checked. A second copy of the list, left behind,
 * builds a page nothing opens.
 *
 * Each link names its language in that language (`name`) for a screen
 * reader, which reads it with that language's rules: "FR" alone is two
 * letters spelt out with the page's.
 *
 * `offer` and `dismiss` are the wording of the "this page exists in your
 * language" hint, each in the language it offers: a French visitor is
 * addressed in French, not in the language of the page they happened to land
 * on. They travel on the switcher links as data attributes because `site.js`
 * reads them from there, which is what keeps the hint out of the four
 * catalogues: it is chrome, not content.
 *
 * No JSON import in this file, on purpose: Node reads it as it is for
 * `check.mjs`, and a JSON import there needs an attribute Vite does not want.
 */
export const LANGUAGES = [
  { code: 'en', label: 'EN', name: 'English', path: '/', offer: 'This page is available in English', dismiss: 'Dismiss' },
  { code: 'fr', label: 'FR', name: 'Français', path: '/fr/', offer: 'Cette page existe en français', dismiss: 'Fermer' },
  { code: 'de', label: 'DE', name: 'Deutsch', path: '/de/', offer: 'Diese Seite gibt es auf Deutsch', dismiss: 'Schließen' },
  { code: 'es', label: 'ES', name: 'Español', path: '/es/', offer: 'Esta página está disponible en español', dismiss: 'Cerrar' },
] as const;

export type Locale = (typeof LANGUAGES)[number]['code'];
