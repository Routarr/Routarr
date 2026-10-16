import type { Page } from '@playwright/test';

import { test, expect, api, openScreen } from './fixtures';
import { SCREENS, SETTINGS_SECTIONS } from './screens';

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

/**
 * Every screen and each section of the settings, with the guide's banners, and
 * the two dialogs a first run opens.
 */
const PATHS = [...SCREENS, '/rules?new=1', '/instances?add=1', ...SETTINGS_SECTIONS];
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
  /**
   * The layout the page had at that moment: the width it was laid out at,
   * whether the drawer breakpoint of `index.css` applied, where the rail sat
   * and how wide the content was. A fault a loaded runner shows now and then
   * says from it whether the page or the reading was out of step.
   */
  layout: string;
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
      const rail = document.querySelector('#sidebar');
      const main = document.querySelector('#main');
      const layout = JSON.stringify({
        viewport: innerWidth,
        client: root.clientWidth,
        scroll: root.scrollWidth,
        scrollX,
        drawer: matchMedia('(max-width: 900px)').matches,
        rail: rail ? getComputedStyle(rail).position : null,
        content: main ? Math.round(main.getBoundingClientRect().width) : null,
      });
      return { wrapped, spilled, layout };
    },
    { selectors: ONE_LINE, sameLine: SAME_LINE },
  );
}

/**
 * The guide comes from the shell's own request, apart from the screen's: read
 * before it lands, a guide that wraps passes unseen in one language and is
 * blamed on another in the next. The pill is drawn from it, found by its class
 * since a dialog open on arrival leaves it inert, and on the dashboard the
 * guide itself is waited for.
 */
async function guideShown(page: Page, path: string): Promise<void> {
  await expect(page.locator('.guide-pill')).toBeVisible();
  if (path === '/') await expect(page.locator('#guide-title')).toBeVisible();
}

/**
 * A layout that follows the width has run once the style has taken the new
 * width, the next frame is drawn and every transition it started has ended.
 *
 * WebKit on a loaded runner can lay out the new width while its media queries
 * still hold the old one, and the page is then read with the rules of 1280px
 * squeezed into 390. A rule of the sweep's own, matching the new width alone,
 * shows when the style has caught up. It is a constructed sheet, which the
 * page's CSP does not govern.
 *
 * Read halfway through a transition, an inset still on its way holds a label
 * on one line it wraps once settled. With motion reduced a transition ends
 * within the frame, so that wait is bounded: one that never ends, left by an
 * earlier spec, cannot hold the sweep.
 */
async function resize(page: Page, width: number): Promise<void> {
  await page.setViewportSize({ width, height: 900 });
  await page.evaluate(async (width) => {
    const probe = new CSSStyleSheet();
    probe.replaceSync(`@media (width: ${width}px) { :root { --sweep-width: ${width}; } }`);
    document.adoptedStyleSheets = [...document.adoptedStyleSheets, probe];
    const caughtUp = () =>
      getComputedStyle(document.documentElement).getPropertyValue('--sweep-width').trim() ===
      String(width);
    const deadline = performance.now() + 5000;
    while (!caughtUp()) {
      if (performance.now() > deadline) throw new Error(`the style never took ${width}px`);
      await new Promise(requestAnimationFrame);
    }
    document.adoptedStyleSheets = document.adoptedStyleSheets.filter((sheet) => sheet !== probe);
    await new Promise(requestAnimationFrame);
    const settling = document
      .getAnimations()
      .filter((animation) => animation instanceof CSSTransition)
      .map((animation) => animation.finished.catch(() => undefined));
    await Promise.race([Promise.all(settling), new Promise((done) => setTimeout(done, 100))]);
  }, width);
}

async function speak(code: string): Promise<void> {
  await api('/settings', {
    method: 'PUT',
    body: JSON.stringify({ settings: { ui_language: code } }),
  });
}

test('every language holds on one line what English does, and nothing spills', async ({ page }) => {
  // Every language over every screen at two widths: some four minutes in
  // Chromium, and WebKit on a CI runner needs about twice that.
  test.setTimeout(600_000);
  // Motion reduced, as the interface honours it: a transition ends within the
  // frame, so a thousand resizes do not each wait out an animation.
  await page.emulateMedia({ reducedMotion: 'reduce' });
  // The guide on screen, its first step done by the fixture's synced instance.
  await api('/onboarding', { method: 'PUT', body: JSON.stringify({ state: 'pending' }) });
  const { languages } = (await api('/localization/languages')) as { languages: { code: string }[] };
  const codes = ['en', ...languages.map((language) => language.code).filter((c) => c !== 'en')];

  const english = new Set<string>();
  const faults: string[] = [];
  let shots = 0;
  for (const code of codes) {
    await speak(code);
    for (const path of PATHS) {
      await resize(page, WIDTHS[0]!);
      await openScreen(page, path);
      await guideShown(page, path);
      if (path.includes('=1')) await expect(page.getByRole('dialog')).toBeVisible();

      for (const width of WIDTHS) {
        await resize(page, width);
        const { wrapped, spilled, layout } = await read(page);
        const where = `${path} at ${width}px`;
        const before = faults.length;
        for (const [key, text] of Object.entries(wrapped)) {
          if (code === 'en') english.add(`${where} ${key}`);
          else if (!english.has(`${where} ${key}`)) {
            faults.push(`${code} ${where}: "${text}" wraps where English holds one line`);
          }
        }
        faults.push(...spilled.map((what) => `${code} ${where}: "${what}" spills out`));
        if (faults.length > before) {
          faults.push(`${code} ${where}: the page was laid out as ${layout}`);
          // A few, taken just after the reading: enough to see the layout
          // without filling the report.
          if (shots++ < 3) {
            await test.info().attach(`${code} ${path} ${width}px`, {
              body: await page.screenshot(),
              contentType: 'image/png',
            });
          }
        }
      }
    }
  }

  expect(faults).toEqual([]);
});
