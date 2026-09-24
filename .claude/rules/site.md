---
paths:
  - "site/**"
---
# Showcase site

## Commands

```bash
npm --prefix site ci           # once
npm --prefix site run build    # Astro -> site/dist
npm --prefix site run check    # astro check (types), then check.mjs over site/dist
node site/verify.mjs           # site/dist in Chromium under the real _headers: CSP, axe, layout
node site/serve.mjs            # preview site/dist on :8788 with the real _headers
node site/icons.mjs            # favicon.ico and the PNG icons from public/assets/favicon.svg
bash site/screenshots/run.sh   # public/assets/og.png, rendered from screenshots/og.html
```

- `check.mjs` and `verify.mjs` read `site/dist`: rebuild after an edit, or they test the
  previous build.
- Preview through `serve.mjs`. `astro dev` and `astro preview` apply no `_headers`, so a page the
  CSP breaks looks fine there.
- `verify.mjs`, `icons.mjs` and the screenshot capture load Playwright from
  `frontend/node_modules` (`FRONTEND_DIR` overrides the path).
- `screenshots/run.sh` also leaves captures in `public/assets/shots/`, which no page shows and
  `check.mjs` refuses: keep `og.png` and delete the captures. `screenshots/og.html` copies the
  English hero headline (`page.h1`) and the palette by hand, so it changes with them.

## Content Security Policy

- `public/_headers` sets `default-src 'none'` with `'self'` sources: nothing is fetched from
  another origin, and no element carries a `style=` attribute.
- The theme bootstrap (`src/theme-bootstrap.ts`) is the one inline script that runs, allowed by
  its hash in `_headers`: an edit to it needs its new `sha256-` there, or the page loses its
  theme. Any other script is a file under `public/assets/` loaded with
  `<script is:inline src="..." defer>`. Astro inlines a short component `<script>` as a module,
  the CSP blocks it, `check.mjs` never sees it, and `verify.mjs` watches for violations on `/`
  alone.

## Catalogues

- Every key goes into all four catalogues, which `check.mjs` requires. Text a reader should get in
  their language comes from `src/i18n/<code>.json`, `aria-label` and CSS `content` included. Text
  written into a component ships in English on the three translated pages, and `check.mjs` catches
  only labels, CSS `content` and a few known phrases.
- A catalogue value is text or whole elements, never the opening tag of the element it sits in.
  A text value carries no HTML entity (the template escapes it a second time), and a value with
  an element such as `<em>` renders with `set:html`. `check.mjs` catches that entity, and an
  escaped `em`, `strong`, `code`, `span` or `br` only when it carries no attribute.
- An expression attribute takes no quotes: `alt={t('key')}`. In Astro, unlike Svelte,
  `alt="{t('key')}"` renders that literal text, and the `alt` check only asks for a non-empty one.
- Internal links come from `pathFor(locale, page)`, which ends in the slash the server serves.

## Layout

- Sections band by position (`main > section:nth-of-type(even)` in `src/styles/site.css`) and the
  gap between two is their padding alone: a new section takes no background class and no trailing
  margin. No check measures either.
- A block earns a framed grid only when its content has two axes. A sequence is a numbered flow,
  and a one-dimensional list a `dl` whose terms are `dt`, not headings.

## Domain

- The domain is written in `astro.config.mjs` (`site`), `public/robots.txt`,
  `public/.well-known/security.txt` and the root `README.md`. `check.mjs` refuses a second origin
  but reads neither `security.txt` nor the README.
