# Showcase site

The public landing page for Routarr: Astro, static, one small deferred script,
and no external request at run time — which is what keeps the CSP at
`default-src 'none'`.

```
site/
├── src/
│   ├── pages/          the routes: / and /[lang]/, plus /404
│   ├── layouts/        Base (the document) and Landing (the page, once)
│   ├── components/     Header, Footer, and one file per section
│   ├── i18n/           en/fr/de/es catalogues, and the lookup that throws
│   └── styles/site.css the whole stylesheet
├── public/             copied verbatim into dist/
│   ├── assets/         favicon.svg, og.png, site.js, shots/*.{avif,webp}
│   ├── _headers        Cloudflare Pages: CSP, security headers, caching
│   └── robots.txt
├── astro.config.mjs
├── serve.mjs           preview server for dist/, applying _headers
├── check.mjs           static checks, against dist/
├── verify.mjs          browser checks under the real CSP
├── icons.mjs           re-renders the raster icons from favicon.svg
└── screenshots/        the harness that regenerates public/assets/shots
```

## Working on it

```bash
npm ci                     # once, in site/
npm run build              # Astro -> site/dist
npx astro check            # types over the components and the catalogue
node serve.mjs             # http://127.0.0.1:8788, serves dist/ with _headers
node check.mjs             # static checks against dist/
node verify.mjs            # browser checks, starts its own server
```

`serve.mjs` parses and applies `_headers`, so the strict Content-Security-Policy
is exercised locally exactly as Cloudflare will serve it. Previewing with any
other static server would hide a CSP that blocks the stylesheet.

### What the checks cover

`check.mjs` — one site origin used consistently, the inline script's CSP hash
still matching, title and description lengths, exactly one `<h1>`, images with
alt text and with declared dimensions that match the real files, in-page links
that resolve, no third-party subresource, the light palette declared identically
in both places it appears, and WCAG AA contrast on both themes.

`verify.mjs` — the page loads with no console error and no CSP violation, talks
to no external host, every image decodes, nothing scrolls sideways at 360, 768,
1024 and 1440 px, the theme toggle works and survives a reload, the skip link is
the first tab stop, and an unknown path returns a rendered 404.

Both run in CI.

## Regenerating the screenshots

```bash
bash site/screenshots/run.sh
```

Starts a throwaway Routarr — its own database, its own fake Radarr, Sonarr and
TMDb, its own ports — seeds a demo library, waits for the first scheduler pass,
then drives Chromium over the real interface and encodes the results to WebP. It
never touches a development instance, so no real library and no real API key can
end up in a published image.

Re-crop a screenshot and `check.mjs` will fail until the `width`/`height` in the
HTML match the new file.

## Translations

One set of components, rendered four times. `src/layouts/Landing.astro` is the
page; `src/pages/index.astro` and `src/pages/[lang]/index.astro` are the routes
that render it as English, French, German and Spanish. `src/i18n/<code>.json`
maps a key to a string, and a translator edits prose — the structure was never
theirs to keep in step, and now it is not even in the same file.

```bash
npm run build              # writes dist/, dist/fr/, dist/de/, dist/es/
npx astro check            # a key English does not have fails here
node check.mjs             # parity, and the usual checks over all four pages
```

Two properties are load-bearing.

**A missing string fails the build.** `useTranslations` falls back to English
and then throws, so a key nothing answers stops the build rather than rendering
an empty element. `Key` is derived from `en.json`, so a key the markup asks for
and English does not have fails the type check before that. `check.mjs` also
compares each catalogue's keys against English in both directions, and refuses
an empty value.

**`{t('key')}` in an attribute must not be quoted.** `alt="{t('x')}"` is a
literal string in Astro, not an expression — it renders every `alt` on the page
as `{t('…')}` without a word of warning. Write `alt={t('x')}`.

**A catalogue value is a string, not a fragment of markup.** Two mistakes are
easy to make and silent. A value must not carry the opening tag it is meant to
sit inside — `<span class="eyebrow">Features`, `<h3>Decide` — or the markup
leaves the template unbalanced; the tag belongs in the component and the value
is just the words. And a value must not carry HTML entities: an `&amp;` is
harmless in raw markup and double-escaped the moment it goes through a template
that escapes.

**The visitor's language is offered, never imposed.** `site.js` reads
`navigator.languages`, and when the browser prefers a language the site speaks
that is not the one on screen, a banner appears with a link to it. The wording
and the close button's label are written in the language being offered and live
on the switcher link itself, as `data-offer` and `data-dismiss`, so there is no
second table to keep in step. A click on any language is remembered and the
banner never argues again. There is deliberately no redirect: it would hijack a
shared URL, flash the wrong language first, and do nothing for a reader without
JavaScript — who, this way, simply never sees a banner. The container ships
empty and hidden, and the link is built at display time, because an `<a>` with
no text is a control with no accessible name and `verify.mjs` is right to
refuse it.

Adding a language means an entry in `LANGUAGES` (`src/i18n/languages.ts`) with
its `offer` and `dismiss` wording, and a `src/i18n/<code>.json`. The route, the
`hreflang` block, the switcher entry, the locale list in `astro.config.mjs`, the
sitemap and the pages `check.mjs` opens all follow from `LANGUAGES` on their
own; `check.mjs` fails until the catalogue is complete.

**The icons.** `public/assets/favicon.svg` is the drawing; the 32x32 PNG, the
180x180 apple-touch-icon and the root `favicon.ico` are rendered from it by
`node site/icons.mjs`. Edit the SVG, re-run the script, commit all four —
`check.mjs` fails if a raster copy is missing or if a `sizes` attribute no
longer matches the pixels behind it. The ICO carries an uncompressed BMP rather
than an embedded PNG, because it exists for the clients that understand the
least.

**The domain.** `routarr.app` stands in for whatever you register:

```bash
grep -rl 'routarr\.app' site/ | xargs sed -i 's|routarr\.app|your-domain|g'
node site/check.mjs
```

It appears in the canonical link, `og:url`, `og:image`, the generated
`sitemap-index.xml` and `robots.txt`. `check.mjs` fails if two different
origins are left behind.

**The repository.** Settled: `https://github.com/Routarr/Routarr`. A fork
changes it the same way as the domain:

```bash
grep -rl 'github.com/Routarr/Routarr' site/ | xargs sed -i 's|github.com/Routarr/Routarr|github.com/you/routarr|g'
```

The compose snippet names `ghcr.io/routarr/routarr:latest`, which is
what `.github/workflows/release.yml` publishes on a `v*` tag. A GHCR package
starts private whatever the repository's visibility, so that snippet only works
for someone who has authenticated to the registry until the package is made
public from its settings page — do that before pointing the world at it.

## Deploying to Cloudflare Pages

Cloudflare builds the site itself, on every push. Connect the repository once —
Workers & Pages → Create → Pages → *Connect to Git* — and set:

| Setting | Value |
|---|---|
| Framework preset | Astro |
| Root directory | `site` |
| Build command | `npm run build && node check.mjs` |
| Build output directory | `dist` |

The checks are part of the build command on purpose: a wrong origin, a stale CSP
hash, a screenshot whose dimensions no longer match, a lone AVIF or a dead link
then fails the deployment instead of being published. `verify.mjs` stays out of
it — it drives Chromium, which the build image does not carry — and runs in CI
and locally instead.

`.node-version` pins Node 24, matching CI and the production image. Cloudflare
reads it; without it the build image picks its own default.

Everything in `public/` is copied into `dist/`, `_headers` included, and Pages
applies it. A custom domain is added under the project's *Custom domains* tab;
Cloudflare issues the certificate, and the `Strict-Transport-Security` header in
`_headers` only makes sense once that is in place.

**`wrangler` is deliberately not used.** `npx wrangler pages deploy dist` works,
from a laptop or from a GitHub Action, but it means a Cloudflare API token kept
as a repository secret and an upload that skips the build entirely. The Git
integration needs no credential of ours, rebuilds from source every time, and
gives a preview deployment per pull request.

Nothing here is Cloudflare-specific beyond `_headers`, which any other static
host ignores harmlessly. On Netlify the same file works as-is; elsewhere the
headers have to be reproduced in that host's own configuration — without them
the page still renders, but without its Content-Security-Policy.
