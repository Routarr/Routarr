import type { Page } from '@playwright/test';

import { test, expect, api } from './fixtures';

/**
 * Every language holds on one line what English holds on one line, and no
 * control spills out of what contains it.
 *
 * A label that fits in English can take twice the room in German, Finnish or
 * Greek. A button never wraps, since `.btn` is `white-space: nowrap`: it
 * widens instead, so for a control the question is whether it stays inside
 * its card, banner or dialog. Measured at the two widths the layout is built
 * for, in a real browser, since jsdom lays nothing out.
 */

/** Every screen, the guide's banners and the two dialogs a first run opens. */
const SCREENS = [
  '/',
  '/rules',
  '/rules?new=1',
  '/rules/tests',
  '/simulation',
  '/media',
  '/history',
  '/overrides',
  '/jobs',
  '/logs',
  '/health',
  '/instances',
  '/instances?add=1',
  '/root-folders',
  '/settings',
  '/settings#routing',
  '/settings#automation',
  '/settings#metadata',
  '/settings#maintenance',
];
const WIDTHS = [1280, 390];

/** Text laid out to hold one line, whatever the language. */
const ONE_LINE = [
  '.page-title',
  '.card-title',
  '.guide-step-title',
  '.guide-count',
  '.badge',
  '.form-label',
  '.nav-label',
];

/** How far apart two children can sit vertically and still read as one line. */
const SAME_LINE = 12;

interface Reading {
  /** One-line text that took more than one, by selector and position. */
  wrapped: Record<string, string>;
  /** Controls reaching past their container, and a page that scrolls sideways. */
  spilled: string[];
}

async function read(page: Page): Promise<Reading> {
  return page.evaluate(
    ({ selectors, sameLine }) => {
      const shown = (el: Element) => {
        const box = el.getBoundingClientRect();
        return box.width > 0 && box.height > 0;
      };
      const wrapped: Record<string, string> = {};
      for (const selector of selectors) {
        document.querySelectorAll<HTMLElement>(selector).forEach((el, index) => {
          if (!shown(el)) return;
          // Text that wraps inside the element: forbidding the wrap makes it
          // shorter.
          const before = el.getBoundingClientRect().height;
          const saved = el.style.whiteSpace;
          el.style.whiteSpace = 'nowrap';
          const after = el.getBoundingClientRect().height;
          el.style.whiteSpace = saved;
          // Children a flex row wrapped, as a badge dropped under its title.
          const centres = [...el.children].filter(shown).map((child) => {
            const box = child.getBoundingClientRect();
            return box.top + box.height / 2;
          });
          const spread = centres.length < 2 ? 0 : Math.max(...centres) - Math.min(...centres);
          if (before > after + 1 || spread > sameLine) {
            wrapped[`${selector}#${index}`] = (el.textContent ?? '').trim().replace(/\s+/g, ' ');
          }
        });
      }

      const spilled: string[] = [];
      document.querySelectorAll<HTMLElement>('.btn, .badge, .guide-pill').forEach((el) => {
        if (!shown(el)) return;
        const box = el.closest(
          '.guide-step, .banner, .modal-content, .card, .page-header, .topbar',
        );
        if (!box) return;
        // Inside a region that scrolls sideways, as a table does on a phone,
        // reaching past the edge is what the scroll is for.
        for (let node = el.parentElement; node && node !== box; node = node.parentElement) {
          const overflow = getComputedStyle(node).overflowX;
          if (overflow === 'auto' || overflow === 'scroll') return;
        }
        const inner = el.getBoundingClientRect();
        const outer = box.getBoundingClientRect();
        if (inner.left < outer.left - 1 || inner.right > outer.right + 1) {
          spilled.push(
            (el.textContent ?? '').trim() || el.getAttribute('aria-label') || '(unnamed)',
          );
        }
      });
      const root = document.documentElement;
      if (root.scrollWidth > root.clientWidth + 1) spilled.push('(the page scrolls sideways)');
      return { wrapped, spilled };
    },
    { selectors: ONE_LINE, sameLine: SAME_LINE },
  );
}

async function speak(code: string): Promise<void> {
  await api('/settings', {
    method: 'PUT',
    body: JSON.stringify({ settings: { ui_language: code } }),
  });
}

test('every language holds on one line what English does, and nothing spills', async ({
  page,
  instanceId,
}) => {
  test.setTimeout(300_000);
  expect(instanceId).toBeTruthy();
  // The guide on screen, its first step done by the fixture's synced instance.
  await api('/onboarding', { method: 'PUT', body: JSON.stringify({ state: 'pending' }) });
  const { languages } = (await api('/localization/languages')) as { languages: { code: string }[] };
  const codes = ['en', ...languages.map((language) => language.code).filter((c) => c !== 'en')];

  const english = new Set<string>();
  const faults: string[] = [];
  try {
    for (const code of codes) {
      await speak(code);
      for (const path of SCREENS) {
        await page.setViewportSize({ width: WIDTHS[0]!, height: 900 });
        await page.goto(path);
        await expect(page.locator('.page-title')).toBeVisible();
        if (path.includes('=1')) await expect(page.getByRole('dialog')).toBeVisible();

        for (const width of WIDTHS) {
          await page.setViewportSize({ width, height: 900 });
          const { wrapped, spilled } = await read(page);
          const where = `${path} at ${width}px`;
          for (const [key, text] of Object.entries(wrapped)) {
            if (code === 'en') english.add(`${where} ${key}`);
            else if (!english.has(`${where} ${key}`)) {
              faults.push(`${code} ${where}: "${text}" wraps where English holds one line`);
            }
          }
          faults.push(...spilled.map((what) => `${code} ${where}: "${what}" spills out`));
        }
      }
    }
  } finally {
    await speak('en');
  }

  expect(faults).toEqual([]);
});
