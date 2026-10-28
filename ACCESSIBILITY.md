# Accessibility

Routarr aims at [WCAG 2.2](https://www.w3.org/TR/WCAG22/) level AA, for its interface and for
its site, [routarr.app](https://routarr.app). This page says how that is checked, what the checks
do not reach, and how to tell us about a barrier.

## How it is checked

The end-to-end suite drives the real interface in Chromium, Firefox and WebKit, and its
[accessibility checks](frontend/e2e/accessibility.spec.ts) hold every screen to these:

- **axe at WCAG 2.2 A and AA**, in the dark and the light theme, and again at the width where
  the navigation becomes a rail.
- **The keyboard**: a walk with Tab reaches every control of every screen, on a wide screen and
  on a phone's width. The first tab stop skips to the content, and a dialog keeps the focus
  inside it, closes on Escape and gives the focus back to what opened it.
- **Names and structure**: every control and every dialog is named, no id is used twice, every
  table has a caption, and headings never skip a level.
- **Forced colours**: a focused field, and the focused item of a menu, stay outlined when the
  system forces its own colours.

Beside them, a unit test measures the contrast of the interface's pairs of colours against AA,
in both themes, and another suite runs the interface in Arabic, right to left. Both the
interface and the site honour a system asking for reduced motion. The site is checked with axe at
WCAG 2.2 A and AA, page by page, before each deployment ([`site/verify.mjs`](site/verify.mjs)).

## What it supports

The interface is made for current Chrome, Edge and Firefox, Firefox's extended support release
included, and Safari of the last two years, as the [README](README.md) says.

## Known limits

The checks above are automated. They find what can be measured: a missing name, a contrast, a
control the keyboard cannot reach. They do not tell whether a screen reader announces a screen in
an order that makes sense, or whether a whole task can be done without sight. Reports from people
who use assistive technology are how those are found.

## Reporting a barrier

Open an [accessibility issue](https://github.com/Routarr/Routarr/issues/new?template=accessibility.yml):
the screen, what you tried, what happened, and the assistive technology and the browser you use,
each with its version. A barrier counts as a bug, whatever the checks above say.
