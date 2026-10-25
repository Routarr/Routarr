import type { Page } from '@playwright/test';

import { test, expect, api, openScreen } from './fixtures';
import { SCREENS, SETTINGS_SECTIONS } from './screens';

/**
 * Every language holds on one line what English does, and no button spills out
 * of its card, banner or dialog. A button widens, and below 560px its label may
 * take two lines rather than spill.
 */

/** Every screen and settings section, and the two dialogs a first run opens. */
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
  /** The page's layout at that moment, reported with a fault. */
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
          // Wrapped text gets shorter once wrapping is forbidden.
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
        // A region that scrolls sideways, as a table on a phone, may overflow.
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
 * The guide loads apart from the screen. Read too early, a guide that wraps is
 * blamed on the next language. Found by class, since an open dialog makes it inert.
 */
async function guideShown(page: Page, path: string): Promise<void> {
  await expect(page.locator('.guide-pill')).toBeVisible();
  if (path === '/') await expect(page.locator('#guide-title')).toBeVisible();
}

/**
 * Resize, then wait until the style has taken the new width and transitions
 * have ended. WebKit can lay out a new width with the old media queries still
 * applied.
 */
async function resize(page: Page, width: number): Promise<void> {
  await page.setViewportSize({ width, height: 900 });
  await page.evaluate(async (width) => {
    // A constructed sheet, outside the page's CSP.
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
  // Every language on every path at each width, in WebKit too.
  test.setTimeout(600_000);
  // Reduced motion keeps each transition within a frame.
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
