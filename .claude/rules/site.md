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
  `frontend/node_modules` through `playwright.mjs` (`FRONTEND_DIR` overrides the path).
- `screenshots/run.sh` writes the captures to `screenshots/captures/`, outside what ships. A page
  that shows one takes its WebP and AVIF pair into `public/assets/shots/`, which `check.mjs`
  pairs and refuses once no page shows it. `screenshots/og.html` copies the English hero headline
  (`page.h1`) and the palette by hand, so it changes with them.

## Content Security Policy

- `public/_headers` sets `default-src 'none'` with `'self'` sources: nothing is fetched from
  another origin, and no element carries a `style=` attribute.
- The theme bootstrap (`src/theme-bootstrap.ts`) is the one inline script that runs, allowed by
  its hash in `_headers`: an edit to it needs its new `sha256-` there, or the page loses its
  theme. Any other script is a file under `public/assets/` loaded with
  `<script is:inline src="..." defer>`. Astro inlines a short component `<script>` as a module
  and the CSP blocks it: `check.mjs` refuses an inline script whose hash `_headers` lacks, and
  `verify.mjs` watches every page for violations.

## Catalogues

- Every key goes into all four catalogues, which `check.mjs` requires. Text a reader should get in
  their language comes from `src/i18n/<code>.json`, `aria-label` and CSS `content` included. Text
  written into a template ships in English on the three translated pages: `check.mjs` refuses it
  outside `NOT_LANGUAGE`. The not-found page is translated too, one per language prefix.
- A catalogue value is text or whole elements, never the opening tag of the element it sits in.
  A text value carries no HTML entity (the template escapes it a second time), and a value with
  an element such as `<em>` renders with `set:html`. `check.mjs` catches that entity and an
  escaped `em`, `strong`, `code`, `span`, `br`, `a` or `kbd`.
- An expression attribute takes no quotes: `alt={t('key')}`. In Astro, unlike Svelte,
  `alt="{t('key')}"` renders that literal text, and the `alt` check only asks for a non-empty one.
- Internal links come from `pathFor(locale, page)`, which ends in the slash the server serves.

## API page

- `src/contract.ts` builds the reference and `/api/v1/openapi.json` from
  `backend/openapi/v1.json`. No operation is written into a page by hand, and the contract's
  prose stays in English under `lang="en"`. A recipe calls documented operations only.

## Layout

- Sections band by position (`main > section:nth-of-type(even)` in `src/styles/site.css`) and the
  gap between two is their padding alone: a new section takes no background class and no trailing
  margin. No check measures either.
- A block earns a framed grid only when its content has two axes. A sequence is a numbered flow,
  and a one-dimensional list a `dl` whose terms are `dt`, not headings.

## Domain

- The domain is written in `astro.config.mjs` (`site`), `public/robots.txt`, `public/llms.txt`,
  `public/.well-known/security.txt` and the root `README.md`. `check.mjs` refuses a second origin
  and reads `security.txt`, not the README.

## llms.txt

- `public/llms.txt` restates the pages, the README and what Routarr does not do, for language
  models (llmstxt.org). `check.mjs` checks its shape and links, never its facts: it changes with
  them.
