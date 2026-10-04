import AxeBuilder from '@axe-core/playwright';
import type { Page } from '@playwright/test';

import { test, expect, api, openScreen, unfold } from './fixtures';
import { moveAkira, seedRows, simulate } from './seed';
import { SCREENS, SETTINGS_SECTIONS } from './screens';
import { screenKey } from '../src/lib/routes';

/**
 * A move History offers to revert. The reset before each test makes the
 * instance again, so the move the seed applied in an earlier test names a
 * title the library no longer holds, and offers none.
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

/**
 * The guard on the seed: a sweep that walks a table with no rows passes on its
 * header alone. A loading row and an empty state are not rows.
 */
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
 * A `.form-label` sitting *next* to a field with no `htmlFor` is decorative: the
 * control has no accessible name and clicking the caption does not focus it.
 *
 * `getByLabel` resolves through the browser's real accessible-name computation,
 * which is why this belongs here and not in a jsdom test: it is the only
 * layer that can tell a visible caption from a programmatic label.
 */
test.describe('form fields carry a programmatic label', () => {
  test('the rule editor', async ({ page }) => {
    await page.goto('/rules');
    await page.getByRole('button', { name: 'New Rule' }).click();

    for (const label of ['Rule name', 'Target category', 'Applies to', 'Priority']) {
      await expect(page.getByLabel(label, { exact: true }), label).toBeVisible();
    }
  });

  test('the instance editor', async ({ page }) => {
    await page.goto('/instances');
    await page.getByRole('button', { name: 'Add instance' }).click();

    for (const label of ['Name', 'Type', 'Base URL', 'API key']) {
      await expect(page.getByLabel(label, { exact: true }), label).toBeVisible();
    }
  });

  test('clicking a caption focuses its field', async ({ page }) => {
    await page.goto('/instances');
    await page.getByRole('button', { name: 'Add instance' }).click();

    // The usability half of a programmatic label: an unassociated caption is
    // inert. Scoped to the dialog, since the page behind it has a column header
    // with the same words.
    await page.getByRole('dialog').getByText('Base URL', { exact: true }).click();
    await expect(page.getByLabel('Base URL', { exact: true })).toBeFocused();
  });

  test('the settings page names every control and links its help text', async ({ page }) => {
    await page.goto('/settings');

    // Every section, not just the one that opens: with the settings grouped
    // into tabs, a field can be correct on screen and unlabelled two tabs away.
    //
    // The wait matters: the page renders a spinner until its settings,
    // categories and languages have all arrived, and `count()` does not
    // retry, so asked too early it returns zero and fails the run at random.
    await expect(page.getByRole('tab').first()).toBeVisible();
    const tabs = await page.getByRole('tab').count();
    expect(tabs).toBeGreaterThan(1);

    let checked = 0;
    for (let index = 0; index < tabs; index += 1) {
      const tab = page.getByRole('tab').nth(index);
      const name = await tab.textContent();
      await tab.click();
      await expect(page.locator('[role=tabpanel] .form-group').first()).toBeVisible();

      // One snapshot of the panel, taken at a single instant, so nothing can
      // shift underneath the assertions.
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
        // The explanatory paragraph is announced with the field rather than lost.
        expect(control.describedBy, `${name}: ${control.id}`).toBe(`${control.id}-help`);
        checked += 1;
      }
    }

    // Guards the guard: a grouping bug that emptied every panel would otherwise
    // pass silently.
    expect(checked).toBeGreaterThanOrEqual(15);
  });
});

test.describe('modal dialogs', () => {
  /**
   * With a plain <div> as the overlay, assistive technology keeps announcing
   * the page behind it, Tab walks straight out, and Escape does nothing. Only a
   * real browser can tell whether the native <dialog> is genuinely modal:
   * jsdom implements neither the top layer nor the focus trap.
   */
  test('a modal is announced as a dialog and closes on Escape', async ({ page }) => {
    await page.goto('/instances');
    await page.getByRole('button', { name: /add instance/i }).click();

    // Announced as a dialog, and named: without the name a screen reader says
    // only "dialog".
    const dialog = page.getByRole('dialog');
    await expect(dialog).toBeVisible();
    await expect(dialog).toHaveAttribute('aria-label', /instance/i);

    // The background is inert: the sidebar link behind the modal refuses the
    // focus even when asked, which is what `showModal()` buys over a styled div.
    const behind = page.locator('.sidebar-nav a').first();
    await behind.focus();
    await expect(behind).not.toBeFocused();

    await page.keyboard.press('Escape');
    await expect(dialog).toBeHidden();
    // Back where it came from, not on `<body>`: from the top of the page a
    // keyboard user tabs through the whole shell to reach the next control.
    await expect(page.getByRole('button', { name: /add instance/i })).toBeFocused();
  });

  test('focus starts inside the modal rather than behind it', async ({ page }) => {
    await page.goto('/instances');
    await page.getByRole('button', { name: /add instance/i }).click();

    const dialog = page.getByRole('dialog');
    await expect(dialog).toBeVisible();

    // Whatever holds focus, it must be inside the dialog: a trap that starts
    // outside itself is not a trap.
    const focusIsInside = await dialog.evaluate((element) =>
      element.contains(document.activeElement),
    );
    expect(focusIsInside).toBe(true);
  });
});

/**
 * Swept rather than sampled: the named tests above check the two editors, which
 * leaves the controls nobody thinks of as a form. The sweep also catches one
 * added later on a screen this file has never heard of.
 */
test('every control on every screen has an accessible name', async ({ page }) => {
  const nameless: string[] = [];

  for (const path of SCREENS) {
    await openWithRows(page, path);

    // The same four sources the browser computes a name from. `labels` covers
    // both a `for=` caption and a control wrapped inside its own `<label>`,
    // which is why a wrapped checkbox is not reported here.
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
 * An `id` names one element. Two controls answering to the same one leave
 * `aria-controls`, `aria-labelledby` and `<label for>` pointing at whichever
 * rendered first. A component with a hard-coded id fails that way the moment
 * it is used twice on a page.
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

/**
 * A table is announced by its caption. Without one a screen reader lands on
 * "table, 7 columns, 12 rows" and nothing says what the rows are.
 */
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

/**
 * The checks above are written for this interface. axe-core is the reference
 * nobody here wrote: WCAG 2.1 A and AA, every screen, so a failure names a rule
 * and not an opinion.
 */
// `best-practice` on top of the standard: it is the tag that carries
// `empty-table-header`, which the WCAG tags do not, so an unnamed column
// header passes under them alone.
const AXE_TAGS = ['wcag2a', 'wcag2aa', 'wcag21a', 'wcag21aa', 'best-practice'];

/**
 * The bound of a test that opens every screen and every Settings section and
 * runs axe or a Tab walk on each. The default bound fits one screen's journey,
 * and a slow runner crosses it on a whole sweep with nothing wrong.
 */
const SWEEP_TIMEOUT = 120_000;

/**
 * Every screen and every Settings section, in both themes: the light palette
 * is a second set of colours, and contrast measured in one says nothing of the
 * other.
 */
for (const theme of ['dark', 'light']) {
  test(`every screen passes axe at WCAG 2.1 AA in the ${theme} theme`, async ({ page }) => {
    test.setTimeout(SWEEP_TIMEOUT);
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
 * A field says it has the focus by its border colour, and forced colours paint
 * every border in one system colour. An outline is drawn in a colour of its
 * own there, so it is what still shows where typing will land.
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
 * A dozen navigation links stand between the top of the page and its content.
 * The first Tab has to offer a way past them, and taking it has to land focus
 * where the content starts.
 */
test('the first tab stop skips to the content', async ({ page }) => {
  await openScreen(page, '/rules');

  await page.keyboard.press('Tab');
  const skip = page.locator(':focus');
  await expect(skip).toHaveAttribute('href', '#main');
  await expect(skip).toBeVisible();

  await page.keyboard.press('Enter');
  await expect(page.locator('main:focus, main:focus-within')).toHaveCount(1);
});

/**
 * A screen changes without a page load, so the tab says which one it is, in
 * the interface language, and a link followed leaves the focus on the heading
 * of the screen it led to, where a screen reader starts reading.
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
    .getByRole('link', { name: new RegExp(`^${strings.Logs}`) })
    .click();
  await expect(page.getByRole('heading', { level: 1 })).toBeFocused();
  await expect(page).toHaveTitle(`${strings.Logs} · Routarr`);
});

/**
 * Every control a pointer can reach, the keyboard reaches, and nothing out of
 * sight takes the focus.
 *
 * Walked with Tab from the top of each screen, at a desktop width and at a
 * phone's, where the navigation is a drawer. A label dressed as a button over
 * a hidden file input, or a closed drawer whose links still take the focus,
 * passes every other check here: both are only seen by walking.
 */
test.describe('the keyboard reaches every control', () => {
  for (const width of [1280, 375]) {
    test(`on every screen at ${width}px`, async ({ page }) => {
      test.setTimeout(SWEEP_TIMEOUT);
      await page.setViewportSize({ width, height: 900 });
      const problems: string[] = [];

      for (const path of [...SCREENS, ...SETTINGS_SECTIONS]) {
        await openWithRows(page, path);

        // The controls a pointer can reach: drawn, enabled, not inert, and not
        // translated out of the viewport (a control past the edge of a table
        // that scrolls sideways is reachable, one past the edge of the page is
        // not). Each is tagged, so the walk can say which it met.
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

        const reached = new Set<number>();
        await page.evaluate(() => (document.activeElement as HTMLElement | null)?.blur());
        for (let press = 0; press < expected.length * 2 + 20; press += 1) {
          await page.keyboard.press('Tab');
          const focused = await page.evaluate(() => {
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
          if (reached.has(index)) break;
          reached.add(index);
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
 * A button that turns disabled while its action runs drops the focus to
 * `<body>`, where a screen reader loses its place and the next Tab starts
 * over. Pressed from the keyboard, each has the focus back once it ends.
 */
test.describe('a button keeps the focus through the action it runs', () => {
  const BUTTONS: { path: string; name: RegExp }[] = [
    { path: '/instances', name: /^Sync now – / },
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
  // A screen reader offers the headings as the outline of the page. An h1
  // followed by an h3 says a level is missing and leaves the reader looking for
  // the section that was skipped.
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
 * Two buttons that do different things must not answer to the same name.
 *
 * A delete button announced as "Delete" on every row, and nothing else, is a
 * destructive action repeated with no way to tell one from another except by
 * looking. Every other check here passes it, because each button does have *a*
 * name.
 *
 * Scoped to table bodies: "Previous" and "Next" appearing once per paginated
 * card is not the same defect, and a rule broad enough to catch that would be
 * turned off within a week.
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
 * The same sweep, for what a page only shows once you ask for it.
 *
 * The screen sweep above walks every path and opens nothing, so every modal is
 * outside it. A dialog is where a screen reader user is most captive: the background is
 * inert, so an unnamed control there is not something they can navigate around.
 */

const MODALS: {
  path: string;
  /** The source file this entry opens. Cross-checked by src/test/modals.test.ts. */
  covers: string;
  open: (page: Page) => Promise<void>;
}[] = [
  {
    path: '/rules',
    covers: 'components/RuleEditor.svelte',
    open: (p) => p.getByRole('button', { name: 'New Rule' }).click(),
  },
  {
    // Reachable from every screen, so the screen it is opened from does not
    // matter. The rules page is simply the one already loaded above.
    path: '/rules',
    covers: 'components/CommandPalette.svelte',
    open: (p) => p.getByRole('button', { name: 'Quick search' }).click(),
  },
  {
    path: '/instances',
    covers: 'pages/Instances.svelte',
    open: (p) => p.getByRole('button', { name: 'Add instance' }).click(),
  },
  {
    path: '/overrides',
    covers: 'pages/Overrides.svelte',
    open: (p) => p.getByRole('button', { name: 'New exception' }).click(),
  },
  {
    path: '/applications',
    covers: 'pages/Applications.svelte',
    open: (p) => p.getByRole('button', { name: 'New key' }).click(),
  },
  {
    path: '/root-folders',
    covers: 'pages/RootFolders.svelte',
    open: (p) => p.getByRole('button', { name: 'New category' }).click(),
  },
  {
    path: '/root-folders',
    covers: 'pages/RootFolders.svelte',
    open: (p) =>
      p
        .getByRole('button', { name: /^Rename category – / })
        .first()
        .click(),
  },
  {
    // The explanation panel, which is the longest of them and the only one
    // built entirely out of what the rule engine returns.
    path: '/media',
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
    open: async (p) => {
      await p
        .getByRole('button', { name: /^Actions – / })
        .first()
        .click();
      await p.getByRole('menuitem', { name: 'Delete' }).click();
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
];

test('every modal names itself and every control inside it', async ({ page }) => {
  const problems: string[] = [];
  await offerRevert();

  for (const [index, modal] of MODALS.entries()) {
    await openScreen(page, modal.path);
    await modal.open(page);

    const dialog = page.getByRole('dialog');
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
