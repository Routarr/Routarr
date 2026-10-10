---
paths:
  - "frontend/**"
---

# Frontend

Components named without a path live in `frontend/src/components/`.

## Shell and routing

- A new screen needs three entries: a lazy import in `ROUTES` in `frontend/src/App.svelte`, a
  route in `ROUTE_GROUPS` in `frontend/src/lib/routes.ts` (the navigation, the command palette and
  `frontend/e2e/screens.ts` read it), and an icon in `ICONS` in `frontend/src/lib/navigation.ts`.
  `frontend/src/test/layout.test.ts` and `frontend/src/lib/routes.test.ts` fail when they disagree.
- A page loads only through those dynamic imports. A static import from the shell adds it to what
  every first visit downloads, whose total `scripts/check-bundle-size.mjs` caps.
- The mount point comes from the `<base href>` the backend injects at run time, so routing is
  `frontend/src/lib/router.svelte.ts` and not a library. Link with `<a href={href('/rules')}>`: a
  bare root-absolute `href` drops the mount point on middle-click and ctrl-click, and a bare
  `#fragment` resolves against the base and reloads the page. Never a `role="link"` element.
- What the chrome polls reads `/status`, and the guide `/onboarding`. `/health` probes every Arr
  and metadata source, a full connect timeout per unreachable host, and answers a probe of the
  last 30 seconds again unless a recheck sends `fresh`. `/health?probe=false` reads the database.
- After a write that can add or remove a warning (an instance, a category mapping, a metadata
  source or key, a settings save or import) or move a step of the guide (a rule, a simulation),
  call `invalidateStatus()` from `frontend/src/lib/status.svelte.ts`. Otherwise the shell corrects
  itself at its next idle poll, and the guide at the next navigation.
- App-wide state is a module-level rune in a `.svelte.ts` file, as `i18n`, `confirm` and `status`
  are. No store library, no context.

## Requests and data

- Every request goes through the `api` object in `frontend/src/api/client.ts`. Nothing in
  `frontend/src/api/` imports Svelte or `frontend/src/lib/`.
- Load with `createAsync` (`frontend/src/lib/async.svelte.ts`) and show its `error` in
  `ErrorBanner`. A failed request is never swallowed into `console.error`, which ESLint allows.
  Pass `deps` as a getter, and hand the loader's `AbortSignal` to the `api` call so a superseded
  or abandoned load is cancelled.
- What an action did goes through `createOutcome` (`frontend/src/lib/outcome.svelte.ts`) and one
  `OutcomeBanner`, never into a load's `error`. Part done and part failed is `warn`, each failed
  item a detail. `frontend/src/test/layout.test.ts` refuses a success drawn anywhere else.
- A timer goes through `poll()` in `frontend/src/lib/poll.svelte.ts`, never a bare `setInterval`:
  it stops in a hidden tab and reloads on return.
- `scripts/check-api-types.py` compares field names, not types, for each pair in its `PAIRS`
  table, and fails on an exported interface in neither `PAIRS` nor `UNPAIRED` (a request body,
  a payload built with `json!`). A `#[serde(flatten)]` struct arrives flat.
- A new setting needs a `KNOWN` entry in `backend/src/services/settings.rs`, which refuses unknown
  keys and holds its default, and a `FIELDS` entry in `frontend/src/lib/settings.ts` listed in
  exactly one `SECTIONS` group. A number field's `range` repeats the backend's bounds, and every
  `fallback` the `KNOWN` default, which `frontend/src/lib/settings.test.ts` reads.
- A value shown with a name carries the code in `value` and the name in `label`. Render
  `label ?? value` and write `value` into a rule: the engine matches the code.

## Language and direction

- Every visible string is a key in `backend/locales/en.json`, read with `t('Key')` from
  `frontend/src/lib/i18n.svelte.ts`. The frontend ships no strings of its own, and a key missing
  from another language falls back to English.
- Join translated clauses with `t('ListSeparator')`, not a comma or `Intl.ListFormat`.
- Sizes, percentages and dates go through the helpers in `frontend/src/api/format.ts`, given
  `i18n.language`. The language is the `ui_language` setting, never the browser's locale, and
  `formatBytes` prints the units the backend prints.
- Arabic ships, so CSS uses logical properties (`margin-inline-start`, `inset-inline-end`,
  `text-align: start`), with a `[dir='rtl']` override where a value has no logical form. The
  direction comes from the backend through `i18n.direction`, never from a list kept here.
- Machine formats (paths, ids, keys, raw timestamps) go in `.mono`, which forces left to right. A
  glyph that means "towards" carries `.dir-aware` so it mirrors. `frontend/e2e/rtl.spec.ts` runs
  the shell and a table in Arabic.

## Styling

- Every style lives in `frontend/src/index.css`, never in a component `<style>` block.
  `frontend/src/test/layout.test.ts` checks that every class the markup names is defined, and
  it counts a `<style>` block as a definition, so nothing refuses one: the rule is a convention.
- An inline `style` is only for a value computed from data, as in `Confidence`, `LibraryFacets`,
  `TableSkeleton`, `ProgressBar` and the size a screen hands `Modal`. The CSP keeps
  `'unsafe-inline'` for these alone (`security_headers` in `backend/src/main.rs`).
- Colours are tokens, never literals: `frontend/src/theme.contrast.test.ts` measures the token
  pairs in its `PAIRS` list against WCAG AA. A light value is written twice, under
  `[data-theme='light']` and in the `prefers-color-scheme: light` block that serves `auto`, and
  the test fails when the two blocks declare different tokens.
- The accent as text or border is `--accent-strong`, as a fill `--accent-primary`. A control's
  boundary is `--border-strong`, a passive separation `--border-subtle`.
- Corners are square: `--radius-xs` (badges), `--radius-sm` (controls) and `--radius-md`
  (surfaces) are 0, still declared where each shape is drawn, since the browser rounds a button
  or a field left to it. `--radius-full` is for the kind, mode and confidence dots, the guide's
  step numbers, the navigation count and the scrollbar thumb only. Shadows mark elevation
  (dialogs, menus, the save bar), never a card.

## Components and markup

- A dialog is `Modal` with a `label`. Each file that renders `<Modal>` is listed by `covers:` in
  `MODALS` in `frontend/e2e/accessibility.spec.ts`, which `frontend/src/test/modals.test.ts`
  enforces.
- A destructive action asks through `askConfirmation` (yes or no) or `ask` (several outcomes) from
  `frontend/src/lib/confirm.svelte.ts`, never `window.confirm`. The message is a rendered string,
  a button label a dictionary key.
- A card title is `CardTitle`, whose `card` id names its entry in `CARD_HELP`
  (`frontend/src/lib/help-content.ts`) and in its screen's `cards`. A heading that cannot hold a
  button, inside a `<summary>`, sits beside its own `HelpToggle`. A new screen needs its `HELP`
  entry and a new term its `TERMS` one. `help.test.ts` and `layout.test.ts` refuse each missing.
- A table sits in `TableRegion` with the same `label` as its `<caption>`. A search box is
  `SearchField`. Icons come from `frontend/src/lib/icons.ts`, never straight from `@lucide/svelte`.
- Pure logic goes in `frontend/src/api/format.ts` or `frontend/src/api/conditions.ts`, where a test
  needs no rendering.
- `frontend/src/test/layout.test.ts` reads every `.svelte` file in `pages/` and `components/` as
  text: every `<form>` is `novalidate`, every `.form-label` has `for=`, every button has a name,
  no `id` repeats, and labelled fields sit in `.form-row` rather than a bare flex row.
- `frontend/e2e/accessibility.spec.ts` sweeps every screen: every control named, no heading level
  skipped, a `<caption>` on every table, axe at WCAG 2.2 A and AA. A row action's `aria-label` is
  the action, a spaced en dash, then the subject (see `frontend/src/pages/Rules.svelte`), so no
  two rows answer to one name.

## Tests and tooling

- Vitest runs in jsdom, which applies no stylesheet and computes no layout. Geometry, overflow and
  pixels are asserted in `frontend/e2e/layout.spec.ts`.
- Render with `renderWithI18n` (`frontend/src/test/render.ts`), seeding only the strings a test
  asserts on. A component that takes a snippet, or a helper that opens an effect, is driven through
  a small harness in `frontend/src/test/`.
- Drive a `<select>` with `userEvent.selectOptions`. A synthetic `change` moves the DOM but not
  `bind:value`, and the test passes against a request never made.
- Answer a confirmation with `answerConfirmation` (`frontend/src/test/confirm.ts`). In e2e, click
  the dialog's real button, never `page.on('dialog')`.
- Coverage counts every source file under `frontend/src/` but the tests, their helpers, `main.ts`
  and `api/types.ts` (`coverage` in `frontend/vite.config.ts`), so a new file without a test
  fails the per-file floor, whatever the aggregate says.
- `npm run test:e2e` builds the release binary and the frontend, then drives Chromium against a
  fake Radarr, or the engine `ROUTARR_E2E_BROWSER` names: CI runs it in Firefox and WebKit too.
  Specs are chosen by tag, never by file: one that needs a sub-path mount carries `@subpath` in
  its title and runs under `npm run test:e2e:base`, one that needs a sign-in mode carries
  `@forms` or `@oidc` and runs under `npm run test:e2e:auth` with a page holding no key, every
  other spec by default.
- A new journey queries by role and label, not by class. The suite runs serially against one
  server, which the `instanceId` fixture resets before every test, so a spec restores any
  setting it changes beyond that reset.
- A spec imports `test`, `expect` and `api` from `frontend/e2e/fixtures.ts`, which opens a session
  with the harness's key. Imported from `@playwright/test`, a page lands on the key gate. A screen
  opens with its `openScreen`, which fails on the not-found page a bare `h1` wait accepts.
- `tsconfig.json` sets `erasableSyntaxOnly` (no `enum`, no `namespace`, no constructor parameter
  properties) and `noUncheckedIndexedAccess`.
- ESLint is type-aware: no floating promise, type-only imports marked `type`, no `console.log`, and
  a `Set` or `Map` mutated in reactive code is a `SvelteSet` or `SvelteMap`. `frontend/e2e/` has
  its own `tsconfig.json`, since the root one excludes it.
