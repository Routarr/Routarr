// @ts-check
import { LANGUAGES } from './src/i18n/languages.ts';
import { defineConfig } from 'astro/config';
import sitemap from '@astrojs/sitemap';

/**
 * The showcase site: four languages, static, and no JavaScript it does not need.
 *
 * `site` is what makes `Astro.url` absolute, so the canonical link, the OpenGraph
 * URL and the `hreflang` block are all derived from one value rather than
 * repeated as literals in four pages, where they drift apart.
 *
 * `prefixDefaultLocale: false` keeps English at `/` and the rest at `/fr/`,
 * `/de/`, `/es/` — the paths already published, already in `sitemap.xml` and
 * already named by every `hreflang`.
 */
export default defineConfig({
  site: 'https://routarr.app',

  i18n: {
    defaultLocale: 'en',
    locales: LANGUAGES.map((language) => language.code),
    routing: { prefixDefaultLocale: false, redirectToDefaultLocale: false },
  },

  build: {
    // One stylesheet rather than one per page: the four pages share every rule,
    // and `_headers` caches `/_astro/*` for a year on the strength of the hash
    // in the filename.
    inlineStylesheets: 'never',
  },

  // The page is content. Nothing here is worth a client-side framework, and the
  // one script is deferred and optional — the site reads and navigates with
  // JavaScript switched off.
  //
  // The sitemap is the exception, and it earns its place. Kept by hand in
  // `public/`, nothing checks it: a fifth language would be left out of it
  // silently, while the routes, the `hreflang` block and the switcher all
  // follow from `LANGUAGES`. Generated from the routes, it cannot disagree
  // with them.
  integrations: [
    sitemap({
      // `/404` is `noindex`; a sitemap that lists it invites a crawl of the one
      // page telling crawlers to go away.
      filter: (page) => !page.includes('/404'),
      // Each entry names its translations, as the pages' `hreflang` block
      // does: a crawler reading the sitemap alone otherwise sees four
      // unrelated pages.
      i18n: {
        defaultLocale: 'en',
        locales: Object.fromEntries(LANGUAGES.map((language) => [language.code, language.code])),
      },
    }),
  ],
});
