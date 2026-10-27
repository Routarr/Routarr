import AxeBuilder from '@axe-core/playwright';
import type { Page } from '@playwright/test';

import { test, expect, api, openScreen, unfold, AXE_TAGS } from './fixtures';
import { moveAkira, seedRows, simulate } from './seed';
import { SCREENS, SETTINGS_SECTIONS } from './screens';
import { screenKey } from '../src/lib/routes';

/**
 * A move History offers to revert. Each test's reset makes the instance again, so a move
 * applied in an earlier test names a title missing from the new library.
 */
async function offerRevert(): Promise<void> {
  const { data: applied } = (await api('/decisions?status=applied&per_page=200')) as {
    data: { revertible: boolean }[];
  };
  if (!applied.some((decision) => decision.revertible)) await moveAkira(await simulate());
}

test.beforeEach(seedRows);

/** A screen once its data has drawn and opened: the sweeps read what the rows carry. */
async function openWithRows(page: Page, path: string): Promise<void> {
  await openScreen(page, path);
  await page.waitForLoadState('networkidle');
  await unfold(page);
}

/** A sweep over a table without rows passes on its header alone, and a loading row is none. */
test('the sweeps meet a row in every table they walk', async ({ page }) => {
  const bare: string[] = [];

  for (const path of SCREENS) {
    await openWithRows(page, path);
    const found = await page.evaluate(() =>
      [...document.querySelectorAll('table')]
        .filter(
          (table) =>
            ![...table.querySelectorAll('tbody tr')].some(
              (row) =>
                row.getAttribute('aria-hidden') !== 'true' &&
                !row.querySelector('.empty-state, [role=status]'),
            ),
        )
        .map((table) => table.querySelector('caption')?.textContent?.trim() || '(no caption)'),
    );
    bare.push(...found.map((table) => `${path}: ${table}`));
  }

  expect(bare).toEqual([]);
});

/**
 * A `.form-label` beside a field without `htmlFor` names nothing, and a click on it focuses
 * nothing. Only a browser can click, so that is what this checks.
 */
test.describe('form fields carry a programmatic label', () => {
  test('the settings page names every control and links its help text', async ({ page }) => {
    await page.goto('/settings');

    // `count()` does not retry, so the tabs are awaited first, or the run fails at random.
    await expect(page.getByRole('tab').first()).toBeVisible();
    const tabs = await page.getByRole('tab').count();
    expect(tabs).toBeGreaterThan(1);

    let checked = 0;
    for (let index = 0; index < tabs; index += 1) {
      const tab = page.getByRole('tab').nth(index);
      const name = await tab.textContent();
      await tab.click();
      await expect(page.locator('[role=tabpanel] .form-group').first()).toBeVisible();

      // One snapshot at a single instant, so nothing shifts under the assertions.
      const controls = await page.evaluate(() =>
        [...document.querySelectorAll('[role=tabpanel] .form-group :is(input, select)')].map(
          (control) => ({
            id: control.id,
            describedBy: control.getAttribute('aria-describedby'),
            labels: document.querySelectorAll(`label[for="${control.id}"]`).length,
          }),
        ),
      );

      expect(controls.length, `${name} has no controls`).toBeGreaterThan(0);
      for (const control of controls) {
        expect(control.id, `${name}: a control has no id to be labelled by`).toBeTruthy();
        expect(control.labels, `${name}: ${control.id} is not labelled`).toBe(1);
        // Only a setting carries its paragraph: the account and backup forms explain in a note.
        if (control.id.startsWith('setting-')) {
          expect((control.describedBy ?? '').split(' '), `${name}: ${control.id}`).toContain(
            `${control.id}-help`,
          );
          checked += 1;
        }
      }
    }

    // Guards the guard: a bug that emptied every panel would otherwise pass.
    expect(checked).toBeGreaterThanOrEqual(15);
  });
});

test.describe('modal dialogs', () => {
  /**
   * Only a browser tells whether the native <dialog> is modal: jsdom has neither the top layer
   * nor the focus trap.
   */
  test('a modal is announced as a dialog and closes on Escape', async ({ page }) => {
    await page.goto('/instances');
    await page.getByRole('button', { name: /add instance/i }).click();

    // Named, or a screen reader says only "dialog".
    const dialog = page.getByRole('dialog');
    await expect(dialog).toBeVisible();
    await expect(dialog).toHaveAccessibleName(/instance/i);

    // The background is inert: the sidebar link behind refuses the focus even when asked.
    const behind = page.locator('.sidebar-nav a').first();
    await behind.focus();
    await expect(behind).not.toBeFocused();

    await page.keyboard.press('Escape');
    await expect(dialog).toBeHidden();
    // Back where it came from, or a keyboard user tabs through the whole shell again.
    await expect(page.getByRole('button', { name: /add instance/i })).toBeFocused();
  });
});

/** Swept rather than sampled, so a control added later on any screen is checked too. */
test('every control on every screen has an accessible name', async ({ page }) => {
  const nameless: string[] = [];

  for (const path of SCREENS) {
    await openWithRows(page, path);

    // `labels` covers both a `for=` caption and a control wrapped in its own `<label>`.
    const found = await page.evaluate(() =>
      [...document.querySelectorAll('input, select, textarea')]
        .filter((e) => (e as HTMLInputElement).type !== 'hidden')
        .filter(
          (e) =>
            !(e as HTMLInputElement).labels?.length &&
            !e.getAttribute('aria-label') &&
            !e.getAttribute('aria-labelledby') &&
            !e.getAttribute('title'),
        )
        .map((e) => `${e.tagName}.${e.className || '(no class)'}`),
    );
    nameless.push(...found.map((what) => `${path} ${what}`));
  }

  expect(nameless).toEqual([]);
});

/**
 * Two elements with one `id` leave `aria-controls`, `aria-labelledby` and `<label for>` on
 * whichever rendered first, as a component with a hard-coded id does when used twice.
 */
test('no screen renders the same id twice', async ({ page }) => {
  const duplicates: string[] = [];

  for (const path of SCREENS) {
    await openWithRows(page, path);
    const found = await page.evaluate(() => {
      const seen = new Map<string, number>();
      for (const el of document.querySelectorAll('[id]')) {
        seen.set(el.id, (seen.get(el.id) ?? 0) + 1);
      }
      return [...seen].filter(([, n]) => n > 1).map(([id, n]) => `${id} ×${n}`);
    });
    duplicates.push(...found.map((what) => `${path} ${what}`));
  }

  expect(duplicates).toEqual([]);
});

/** Without a caption a screen reader says "table, 7 columns" and nothing of what the rows are. */
test('every table on every screen carries a caption', async ({ page }) => {
  const bare: string[] = [];

  for (const path of SCREENS) {
    await openWithRows(page, path);
    const found = await page.evaluate(() =>
      [...document.querySelectorAll('table')]
        .filter((table) => !table.querySelector('caption')?.textContent?.trim())
        .map(
          (table) =>
            `${table.className || 'table'} (${table.querySelectorAll('th').length} columns)`,
        ),
    );
    bare.push(...found.map((what) => `${path} ${what}`));
  }

  expect(bare).toEqual([]);
});

/** A sweep of every screen and section outlasts the default bound on a slow runner. */
const SWEEP_TIMEOUT = 120_000;

/** The Tab walk presses once per control on every screen and waits two frames each time. */
const WALK_TIMEOUT = 2 * SWEEP_TIMEOUT;

/**
 * Both themes, as contrast measured in one says nothing of the other, and once more at the
 * rail's width, where the navigation is drawn otherwise.
 */
for (const [theme, width] of [
  ['dark', 1280],
  ['light', 1280],
  ['dark', 1000],
] as const) {
  test(`every screen passes axe at WCAG 2.2 AA in the ${theme} theme at ${width}px`, async ({
    page,
  }) => {
    test.setTimeout(SWEEP_TIMEOUT);
    await page.setViewportSize({ width, height: 900 });
    await api('/settings', {
      method: 'PUT',
      body: JSON.stringify({ settings: { ui_theme: theme } }),
    });
    const violations: string[] = [];

    for (const path of [...SCREENS, ...SETTINGS_SECTIONS]) {
      await openWithRows(page, path);
      const results = await new AxeBuilder({ page }).withTags(AXE_TAGS).analyze();
      for (const v of results.violations) {
        violations.push(`${path} ${v.id} (${v.impact}, ${v.nodes.length} node(s)): ${v.help}`);
      }
    }

    expect(violations).toEqual([]);
  });
}

/**
 * Forced colours paint every border in one system colour, so only an outline still shows
 * where the focus is.
 */
test('a focused field is outlined when the system forces its colours', async ({ page }) => {
  await page.emulateMedia({ forcedColors: 'active' });
  await page.goto('/instances');
  await page.getByRole('button', { name: 'Add instance' }).click();

  const field = page.getByLabel('Name', { exact: true });
  await field.focus();
  const outline = await field.evaluate((element) => {
    const style = getComputedStyle(element);
    return { style: style.outlineStyle, width: style.outlineWidth, color: style.outlineColor };
  });

  expect(outline.style).not.toBe('none');
  expect(parseFloat(outline.width)).toBeGreaterThan(0);
  expect(outline.color).not.toBe('rgba(0, 0, 0, 0)');
});

/**
 * A row's menu is walked with the arrows, and the item Enter would run shows which it is,
 * in forced colours too.
 */
for (const forced of ['none', 'active'] as const) {
  test(`the focused item of a row's menu is outlined, forced colours ${forced}`, async ({
    page,
  }) => {
    await page.emulateMedia({ forcedColors: forced });
    await openScreen(page, '/instances');
    const actions = page.getByRole('button', { name: /^Actions – / }).first();
    await actions.focus();
    await page.keyboard.press('ArrowDown');

    const item = page.getByRole('menuitem').first();
    await expect(item).toBeFocused();
    const outline = await item.evaluate((element) => {
      const style = getComputedStyle(element);
      return { style: style.outlineStyle, width: style.outlineWidth };
    });
    expect(outline.style).not.toBe('none');
    expect(parseFloat(outline.width)).toBeGreaterThan(0);
  });
}

/** The first Tab skips the dozen navigation links and lands the focus on the content. */
test('the first tab stop skips to the content', async ({ page }) => {
  await openScreen(page, '/rules');

  await page.keyboard.press('Tab');
  const skip = page.locator(':focus');
  await expect(skip).toHaveAttribute('href', '#main');
  await expect(skip).toBeVisible();

  await page.keyboard.press('Enter');
  await expect(page.locator('main:focus, main:focus-within')).toHaveCount(1);
  // Past the top bar, which is the page's banner beside `main`, not inside it.
  await page.keyboard.press('Tab');
  await expect(page.locator('.page-container :focus')).toHaveCount(1);
  await expect(page.getByRole('banner')).toHaveCount(1);
});

/**
 * A screen changes without a page load, so the tab names it, and a followed link leaves the
 * focus on its heading, where a screen reader starts reading.
 */
test('each screen names itself in the tab and takes the focus it was reached with', async ({
  page,
}) => {
  await api('/settings', {
    method: 'PUT',
    body: JSON.stringify({ settings: { ui_language: 'fr' } }),
  });
  const { strings } = (await api('/localization')) as { strings: Record<string, string> };

  for (const path of SCREENS) {
    await openScreen(page, path);
    await expect(page).toHaveTitle(`${strings[screenKey(path)]} · Routarr`);
  }
  await page.goto('/no-such-screen');
  await expect(page).toHaveTitle(`${strings.NotFoundTitle} · Routarr`);

  await openScreen(page, '/');
  await page
    .getByRole('navigation', { name: strings.MainNavigation })
    .getByRole('link', { name: new RegExp(`^${strings.MoveLog}`) })
    .click();
  await expect(page.getByRole('heading', { level: 1 })).toBeFocused();
  await expect(page).toHaveTitle(`${strings.MoveLog} · Routarr`);
});

/** The key screen, which the sweep never reaches: its page holds a session from the start. */
test('the key screen passes axe', async ({ browser }) => {
  // A cold context's first page can take most of the default bound in Firefox on CI.
  test.setTimeout(60_000);
  const context = await browser.newContext();
  const page = await context.newPage();
  await page.goto('/', { waitUntil: 'domcontentloaded' });
  await expect(page.getByLabel('Routarr API key')).toBeVisible();

  const results = await new AxeBuilder({ page }).withTags(AXE_TAGS).analyze();
  expect(results.violations.map((v) => `${v.id}: ${v.help}`)).toEqual([]);
  await context.close();
});

/**
 * On the rail of icons, an entry with a count is named by its destination first, which is
 * what a voice command says and a screen reader reads.
 */
test('a rail entry with a count is named by its destination first', async ({ page }) => {
  await page.setViewportSize({ width: 1000, height: 800 });
  await openScreen(page, '/');

  const nav = page.getByRole('navigation', { name: 'Main navigation' });
  await expect(nav.getByRole('link', { name: /^Simulation .+: \d+$/ })).toBeVisible();
});

/**
 * Walked with Tab at a desktop width and a phone's: a label dressed as a button over a hidden
 * input, or a closed drawer whose links take the focus, shows only when walked.
 */
test.describe('the keyboard reaches every control', () => {
  for (const width of [1280, 375]) {
    test(`on every screen at ${width}px`, async ({ page }) => {
      test.setTimeout(WALK_TIMEOUT);
      await page.setViewportSize({ width, height: 900 });
      const problems: string[] = [];

      for (const path of [...SCREENS, ...SETTINGS_SECTIONS]) {
        await openWithRows(page, path);

        // What a pointer reaches: drawn, enabled, not inert, in view or in a sideways scroller.
        const expected = await page.evaluate(() => {
          const scrollsSideways = (el: Element) => {
            for (let up = el.parentElement; up; up = up.parentElement) {
              const overflow = getComputedStyle(up).overflowX;
              if (/auto|scroll/.test(overflow) && up.scrollWidth > up.clientWidth) return true;
            }
            return false;
          };
          const selector =
            'a[href], button, input, select, textarea, summary, label.btn, [tabindex]';
          const controls = [...document.querySelectorAll<HTMLElement>(selector)].filter((el) => {
            const box = el.getBoundingClientRect();
            const outside = box.right <= 0 || box.left >= innerWidth;
            return (
              el.checkVisibility() &&
              !el.matches(':disabled') &&
              el.getAttribute('tabindex') !== '-1' &&
              !el.closest('[inert]') &&
              !(outside && !scrollsSideways(el))
            );
          });
          return controls.map((el, index) => {
            el.dataset.keyboardSweep = String(index);
            const name = el.getAttribute('aria-label') ?? el.textContent?.trim().slice(0, 40);
            return `${el.tagName.toLowerCase()} "${name || el.id}"`;
          });
        });

        // Focused on the first control, since after a blur each browser starts somewhere else.
        const reached = new Set<number>([0]);
        let last = 0;
        await page.locator('[data-keyboard-sweep="0"]').focus();
        for (let press = 0; press < expected.length * 2 + 20; press += 1) {
          await page.keyboard.press('Tab');
          const focused = await page.evaluate(async () => {
            // WebKit scrolls a newly focused control into view a frame later.
            await new Promise((settled) =>
              requestAnimationFrame(() => requestAnimationFrame(settled)),
            );
            const el = document.activeElement as HTMLElement | null;
            if (!el || el === document.body) return null;
            const box = el.getBoundingClientRect();
            const seen =
              el.checkVisibility() &&
              box.right > 0 &&
              box.left < innerWidth &&
              box.bottom > 0 &&
              box.top < innerHeight;
            const name = el.getAttribute('aria-label') ?? el.textContent?.trim().slice(0, 40);
            return {
              tag: el.dataset.keyboardSweep,
              name: `${el.tagName.toLowerCase()} "${name}"`,
              seen,
            };
          });
          if (!focused) break;
          if (!focused.seen) problems.push(`${path}: ${focused.name} takes the focus out of sight`);
          if (focused.tag === undefined) continue;
          const index = Number(focused.tag);
          // A date field keeps the focus through its day, month and year: the walk goes on.
          if (index === last) continue;
          if (reached.has(index)) break;
          reached.add(index);
          last = index;
        }
        expected.forEach((name, index) => {
          if (!reached.has(index)) problems.push(`${path}: ${name} cannot be reached`);
        });
      }

      expect([...new Set(problems)]).toEqual([]);
    });
  }
});

/**
 * A button disabled while its action runs drops the focus to `<body>`, so each one pressed
 * from the keyboard has the focus back once it ends.
 */
test.describe('a button keeps the focus through the action it runs', () => {
  const BUTTONS: { path: string; name: RegExp }[] = [
    { path: '/instances', name: /^Sync now – Radarr$/ },
    { path: '/instances', name: /^Sync all$/ },
    { path: '/simulation', name: /^Run simulation$/ },
    { path: '/settings#maintenance', name: /^Back up now$/ },
  ];
  for (const { path, name } of BUTTONS) {
    test(`${name.source} on ${path}`, async ({ page }) => {
      await openScreen(page, path);
      const button = page.getByRole('button', { name });
      await button.focus();
      await page.keyboard.press('Enter');

      await expect(button).toBeEnabled();
      await expect(button).toBeFocused();
    });
  }
});

test('every screen nests its headings without skipping a level', async ({ page }) => {
  // A skipped heading level sends a screen reader looking for a missing section.
  const jumps: string[] = [];

  for (const path of SCREENS) {
    await openWithRows(page, path);

    const levels = await page.evaluate(() =>
      [...document.querySelectorAll('h1, h2, h3, h4, h5, h6')].map((h) => ({
        level: Number(h.tagName[1]),
        text: (h.textContent ?? '').trim().slice(0, 40),
      })),
    );

    levels.forEach((heading, i) => {
      const previous = levels[i - 1];
      if (previous && heading.level - previous.level > 1) {
        jumps.push(
          `${path}: h${previous.level} "${previous.text}" → h${heading.level} "${heading.text}"`,
        );
      }
    });

    // A page with no h1 has no name in the outline at all.
    expect(
      levels.filter((h) => h.level === 1),
      `${path} has no single h1`,
    ).toHaveLength(1);
  }

  expect(jumps).toEqual([]);
});

/**
 * Two row actions with one name, as "Delete" on every row, can only be told apart by looking.
 * Table bodies only: a paginated card's "Previous" and "Next" repeating is another matter.
 */
test('no two row actions in a table answer to the same name', async ({ page }) => {
  const clashes: string[] = [];

  for (const path of SCREENS) {
    await openWithRows(page, path);

    const found = await page.evaluate(() => {
      const out: string[] = [];
      for (const body of document.querySelectorAll('tbody')) {
        const names = [...body.querySelectorAll('button')].map(
          (b) => b.getAttribute('aria-label') ?? (b.textContent ?? '').trim(),
        );
        const seen = new Map<string, number>();
        names.filter(Boolean).forEach((n) => seen.set(n, (seen.get(n) ?? 0) + 1));
        for (const [name, count] of seen) if (count > 1) out.push(`${name} ×${count}`);
      }
      return out;
    });

    clashes.push(...found.map((what) => `${path}: ${what}`));
  }

  expect(clashes).toEqual([]);
});

/**
 * The same sweep for every dialog, which the screen sweep opens none of. Its background is
 * inert, so an unnamed control there cannot be navigated around.
 */

const MODALS: {
  path: string;
  /** The source file this entry opens. Cross-checked by src/test/modals.test.ts. */
  covers: string;
  /** The name of the dialog swept, for one opened above another. */
  named?: string;
  /** `alertdialog` for a question that interrupts, a plain dialog otherwise. */
  role?: 'dialog' | 'alertdialog';
  open: (page: Page) => Promise<void>;
}[] = [
  {
    path: '/rules',
    covers: 'components/RuleEditor.svelte',
    open: (p) => p.getByRole('button', { name: 'New Rule' }).click(),
  },
  {
    path: '/rules',
    covers: 'components/TemplateDialog.svelte',
    // The rules seeded fold the gallery, which the header's Templates unfolds.
    open: async (p) => {
      await p.getByRole('button', { name: 'Templates', exact: true }).click();
      await p.getByRole('button', { name: 'Add – Documentaries' }).click();
    },
  },
  {
    // Reachable from every screen. The rules page is the one already loaded.
    path: '/rules',
    covers: 'components/CommandPalette.svelte',
    open: (p) => p.getByRole('button', { name: 'Quick search' }).click(),
  },
  {
    // The help of the screen on display, from the top bar of every screen.
    path: '/rules',
    covers: 'components/HelpPanel.svelte',
    open: (p) => p.getByRole('button', { name: 'Help', exact: true }).click(),
  },
  {
    // The version at the foot of the navigation, on every screen.
    path: '/rules',
    covers: 'components/AboutDialog.svelte',
    open: (p) => p.getByRole('button', { name: /^Routarr v/ }).click(),
  },
  {
    path: '/instances',
    covers: 'pages/Instances.svelte',
    open: (p) => p.getByRole('button', { name: 'Add instance' }).click(),
  },
  {
    path: '/exceptions',
    covers: 'pages/Exceptions.svelte',
    open: (p) => p.getByRole('button', { name: 'New exception' }).click(),
  },
  {
    path: '/applications',
    covers: 'pages/Applications.svelte',
    open: (p) => p.getByRole('button', { name: 'New key' }).click(),
  },
  {
    path: '/categories',
    covers: 'pages/Categories.svelte',
    open: (p) => p.getByRole('button', { name: 'New category' }).click(),
  },
  {
    path: '/categories',
    covers: 'pages/Categories.svelte',
    open: (p) =>
      p
        .getByRole('button', { name: /^Rename category – / })
        .first()
        .click(),
  },
  {
    // The longest panel, built entirely from what the rule engine returns.
    path: '/library',
    covers: 'components/ExplanationModal.svelte',
    open: (p) =>
      p
        .getByRole('button', { name: /^Why\?/ })
        .first()
        .click(),
  },
  {
    // A confirmation, the one a destructive action puts in front of everybody.
    path: '/rules',
    covers: 'components/ConfirmDialog.svelte',
    role: 'alertdialog',
    open: async (p) => {
      await p
        .getByRole('button', { name: /^Actions – / })
        .first()
        .click();
      await p.getByRole('menuitem', { name: 'Delete' }).click();
    },
  },
  {
    // The proof a session gives before a key is made, above the dialog that asked for it.
    path: '/applications',
    covers: 'components/ProofDialog.svelte',
    named: 'Confirm with the API key',
    open: async (p) => {
      await p.getByRole('button', { name: 'New key' }).click();
      await p.getByRole('dialog').getByLabel('Name').fill('proof');
      await p.getByRole('dialog').getByRole('button', { name: 'Create a key' }).click();
    },
  },
  {
    // Offered on the move `offerRevert` makes sure of.
    path: '/history',
    covers: 'pages/History.svelte',
    open: (p) =>
      p
        .getByRole('button', { name: /^Revert – / })
        .first()
        .click(),
  },
  {
    // The revert of many, behind the header's menu, on the same move.
    path: '/history',
    covers: 'pages/History.svelte',
    open: async (p) => {
      await p.getByRole('button', { name: 'Actions', exact: true }).click();
      await p.getByRole('menuitem', { name: /^Revert every move listed/ }).click();
    },
  },
  // Last: the draft it leaves holds a condition, which a navigation would ask about.
  {
    // A value picker whose query matches nothing left to add: the note beside the list.
    path: '/rules',
    covers: 'components/RuleEditor.svelte',
    open: async (p) => {
      await p.getByRole('button', { name: 'New Rule' }).click();
      await p.getByRole('button', { name: 'Add a condition – Conditions', exact: true }).click();
      await p.getByRole('menuitem', { name: /^Genre contains/ }).click();
      const picker = p.getByRole('combobox', { name: 'Genre contains', exact: true });
      await picker.click();
      await p.getByRole('option', { name: 'Animation' }).click();
      await picker.fill('animation');
      // The note, not its region: drawn in an absolute list, the region has no box.
      await p.getByRole('status').getByText('No value matches').waitFor();
    },
  },
];

test('every modal names itself and every control inside it', async ({ page }) => {
  const problems: string[] = [];
  await offerRevert();

  for (const [index, modal] of MODALS.entries()) {
    await openScreen(page, modal.path);
    await modal.open(page);

    const dialog = page.getByRole(modal.role ?? 'dialog', modal.named ? { name: modal.named } : {});
    await expect(dialog, `${modal.path} #${index} did not open`).toBeVisible();

    const found = await dialog.evaluate((root) => {
      const named = (e: Element) =>
        (e as HTMLInputElement).labels?.length ||
        e.getAttribute('aria-label') ||
        e.getAttribute('aria-labelledby') ||
        e.getAttribute('title');

      const nameless = [...root.querySelectorAll('input, select, textarea')]
        .filter((e) => (e as HTMLInputElement).type !== 'hidden')
        .filter((e) => !named(e))
        .map((e) => `unnamed ${e.tagName}.${e.className || '(no class)'}`);

      // A button with neither text nor a label is a shape with no meaning.
      const mute = [...root.querySelectorAll('button')]
        .filter((e) => !(e.textContent ?? '').trim() && !named(e))
        .map((e) => `unnamed BUTTON.${e.className || '(no class)'}`);

      // The dialog itself: without a name it is announced as "dialog".
      const anonymous = named(root) ? [] : ['the dialog carries no accessible name'];

      const levels = [...root.querySelectorAll('h1, h2, h3, h4, h5, h6')].map((h) =>
        Number(h.tagName[1]),
      );
      const skips = levels
        .map((level, i) => {
          const previous = levels[i - 1];
          return previous !== undefined && level - previous > 1 ? `h${previous} → h${level}` : '';
        })
        .filter(Boolean);

      return [...nameless, ...mute, ...anonymous, ...skips];
    });

    problems.push(...found.map((what) => `${modal.path} #${index}: ${what}`));

    // The dialog as axe reads it: the sweep of the screens never sees one open.
    const results = await new AxeBuilder({ page })
      .include('dialog[open]')
      .withTags(AXE_TAGS)
      .analyze();
    for (const v of results.violations) {
      problems.push(`${modal.path} #${index}: ${v.id} (${v.nodes.length} node(s)): ${v.help}`);
    }
  }

  expect(problems).toEqual([]);
});
