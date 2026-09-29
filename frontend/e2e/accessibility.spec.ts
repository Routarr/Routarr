import AxeBuilder from '@axe-core/playwright';
import type { Page } from '@playwright/test';

import { test, expect, api } from './fixtures';
import { SCREENS, SETTINGS_SECTIONS } from './screens';
import { screenKey } from '../src/lib/routes';

/**
 * Every field was captioned by a `.form-label` sitting *next* to it with no
 * `htmlFor`, so the caption was decorative: the control had no accessible name
 * and clicking the caption did not focus it.
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

    // The usability half of the same fix: an unassociated caption is inert.
    // Scoped to the caption element — the page behind the modal has a column
    // header with the same words.
    await page.locator('label[for="instances-base-url"]').click();
    await expect(page.getByLabel('Base URL', { exact: true })).toBeFocused();
  });

  test('the settings page names every control and links its help text', async ({ page }) => {
    await page.goto('/settings');

    // Every section, not just the one that opens: grouping the settings into
    // tabs made it possible for a field to be correct on screen and unlabelled
    // two tabs away.
    //
    // The wait matters: the page renders a spinner until its settings,
    // categories and languages have all arrived, and `count()` does not
    // retry — asking too early returned zero and failed the run at random.
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
   * real browser can tell whether the native <dialog> is genuinely modal —
   * jsdom implements neither the top layer nor the focus trap.
   */
  test('a modal is announced as a dialog and closes on Escape', async ({ page }) => {
    await page.goto('/instances');
    await page.getByRole('button', { name: /add instance/i }).click();

    // Announced as a dialog, and named — without the name a screen reader says
    // only "dialog".
    const dialog = page.getByRole('dialog');
    await expect(dialog).toBeVisible();
    await expect(dialog).toHaveAttribute('aria-label', /instance/i);

    // The background is inert: the sidebar link behind the modal cannot be
    // reached, which is what `showModal()` buys over a styled div.
    await expect(page.locator('.sidebar-nav a').first()).not.toBeFocused();

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
    const focusIsInside = await page.evaluate(() => {
      const dialog = document.querySelector('dialog[open]');
      return !!dialog && dialog.contains(document.activeElement);
    });
    expect(focusIsInside).toBe(true);
  });
});

/**
 * Wait for the screen itself, not merely for the URL.
 *
 * Each route is its own chunk, so `goto` returns with the fallback on screen and
 * the page's own markup still in flight. A sweep measuring then sees the spinner
 * and reports a missing `h1` on a page that has one.
 */
async function ready(page: Page, path: string) {
  await page.goto(path);
  await page.locator('h1').first().waitFor({ state: 'visible' });
}

/**
 * Swept rather than sampled: the named tests above check the two editors, which
 * leaves the controls nobody thinks of as a form — and catches the one added
 * later on a screen this file has never heard of.
 */
test('every control on every screen has an accessible name', async ({ page, instanceId }) => {
  expect(instanceId).toBeTruthy();

  // Without a rule the simulation proposes nothing, so the row checkboxes this
  // test exists for would never render and it would pass on an empty table.
  await api('/rules', {
    method: 'POST',
    body: JSON.stringify({
      name: 'Everything japanese',
      target_category: 'anime',
      media_type: 'movie',
      priority: 10,
      enabled: true,
      condition_logic: 'any',
      conditions: [{ type: 'title_contains', value: ['akira', 'totoro', 'perfect blue'] }],
      exclusions: [],
    }),
  });

  const nameless: string[] = [];

  for (const path of SCREENS) {
    await ready(page, path);

    if (path === '/simulation') {
      await page.getByRole('button', { name: /run simulation/i }).click();
      await expect(page.locator('tbody input[type=checkbox]').first()).toBeVisible();
    }

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
 * rendered first — the failure a component with a hard-coded id produces the
 * moment it is used twice on a page.
 */
test('no screen renders the same id twice', async ({ page, instanceId }) => {
  expect(instanceId).toBeTruthy();
  const duplicates: string[] = [];

  for (const path of SCREENS) {
    await ready(page, path);
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
test('every table on every screen carries a caption', async ({ page, instanceId }) => {
  expect(instanceId).toBeTruthy();
  const bare: string[] = [];

  for (const path of SCREENS) {
    await ready(page, path);
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
 * The checks above are the ones this interface taught us to write. axe-core is
 * the referential nobody here wrote: WCAG 2.1 A and AA, every screen, so a
 * failure names a rule and not an opinion.
 */
test('every screen passes axe at WCAG 2.1 AA', async ({ page, instanceId }) => {
  expect(instanceId).toBeTruthy();
  const violations: string[] = [];

  for (const path of SCREENS) {
    await ready(page, path);
    const results = await new AxeBuilder({ page })
      // `best-practice` on top of the standard: it is the tag that carries
      // `empty-table-header`, which the WCAG tags do not, and an unnamed
      // column header was exactly what shipped under them.
      .withTags(['wcag2a', 'wcag2aa', 'wcag21a', 'wcag21aa', 'best-practice'])
      .analyze();
    for (const v of results.violations) {
      violations.push(`${path} ${v.id} (${v.impact}, ${v.nodes.length} node(s)): ${v.help}`);
    }
  }

  expect(violations).toEqual([]);
});

/**
 * A dozen navigation links stand between the top of the page and its content.
 * The first Tab has to offer a way past them, and taking it has to land focus
 * where the content starts.
 */
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

test('the first tab stop skips to the content', async ({ page, instanceId }) => {
  expect(instanceId).toBeTruthy();
  await ready(page, '/rules');

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
  instanceId,
}) => {
  expect(instanceId).toBeTruthy();
  await api('/settings', {
    method: 'PUT',
    body: JSON.stringify({ settings: { ui_language: 'fr' } }),
  });
  const { strings } = (await api('/localization')) as { strings: Record<string, string> };

  for (const path of SCREENS) {
    await ready(page, path);
    await expect(page).toHaveTitle(`${strings[screenKey(path)]} · Routarr`);
  }
  await ready(page, '/no-such-screen');
  await expect(page).toHaveTitle(`${strings.NotFoundTitle} · Routarr`);

  await ready(page, '/');
  await page
    .getByRole('navigation', { name: strings.MainNavigation })
    .getByRole('link', { name: new RegExp(`^${strings.Logs}`) })
    .click();
  await expect(page.getByRole('heading', { level: 1 })).toBeFocused();
  await expect(page).toHaveTitle(`${strings.Logs} · Routarr`);

  await api('/settings', {
    method: 'PUT',
    body: JSON.stringify({ settings: { ui_language: 'en' } }),
  });
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
    test(`on every screen at ${width}px`, async ({ page, instanceId }) => {
      expect(instanceId).toBeTruthy();
      await page.setViewportSize({ width, height: 900 });
      const problems: string[] = [];

      for (const path of [...SCREENS, ...SETTINGS_SECTIONS]) {
        await ready(page, path);
        await page.waitForLoadState('networkidle');

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
    test(`${name.source} on ${path}`, async ({ page, instanceId }) => {
      expect(instanceId).toBeTruthy();
      await ready(page, path);
      const button = page.getByRole('button', { name });
      await button.focus();
      await page.keyboard.press('Enter');

      await expect(button).toBeEnabled();
      await expect(button).toBeFocused();
    });
  }
});

test('every screen nests its headings without skipping a level', async ({ page, instanceId }) => {
  expect(instanceId).toBeTruthy();

  // A screen reader offers the headings as the outline of the page. An h1
  // followed by an h3 says a level is missing and leaves the reader looking for
  // the section that was skipped — four screens put their first card straight
  // under the page title, three levels deep in markup and two in the outline.
  const jumps: string[] = [];

  for (const path of SCREENS) {
    await ready(page, path);

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
 * Every row of the overrides table carried a delete button announced as
 * "Delete", and nothing else — a destructive action, repeated, with no way to
 * tell one from another except by looking. History had already been fixed to
 * say "Revert — Akira"; the other three tables had not, and no test could see
 * it because each button did have *a* name.
 *
 * Scoped to table bodies: "Previous" and "Next" appearing once per paginated
 * card is not the same defect, and a rule broad enough to catch that would be
 * turned off within a week.
 */
test('no two row actions in a table answer to the same name', async ({ page, instanceId }) => {
  expect(instanceId).toBeTruthy();

  // Two rows, or this proves nothing: a one-row table cannot hold a clash, and
  // the seeded library has exactly one rule and no overrides. The names differ
  // on purpose — two rows called the same thing are ambiguous however they are
  // labelled, and that is a different defect from labelling them all "Delete".
  for (const name of ['Japanese animation', 'Everything else']) {
    await api('/rules', {
      method: 'POST',
      body: JSON.stringify({
        name,
        target_category: 'anime',
        media_type: 'movie',
        priority: name === 'Japanese animation' ? 10 : 20,
        enabled: true,
        condition_logic: 'any',
        conditions: [{ type: 'title_contains', value: ['akira'] }],
        exclusions: [],
      }),
    });
  }

  const clashes: string[] = [];

  for (const path of SCREENS) {
    await ready(page, path);

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
  /** Run before navigating, when the dialog needs the application to have state. */
  prepare?: (page: Page) => Promise<void>;
  open: (page: Page) => Promise<void>;
}[] = [
  {
    path: '/rules',
    covers: 'components/RuleEditor.svelte',
    open: (p) => p.getByRole('button', { name: 'New Rule' }).click(),
  },
  {
    // Reachable from every screen, so the screen it is opened from does not
    // matter; the rules page is simply the one already loaded above.
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
    // built entirely out of backend prose.
    path: '/media',
    covers: 'components/ExplanationModal.svelte',
    open: (p) =>
      p
        .getByRole('button', { name: /^Why\?/ })
        .first()
        .click(),
  },
  {
    // A confirmation, which is the newest of them and the one a destructive
    // action puts in front of everybody.
    path: '/rules',
    covers: 'components/ConfirmDialog.svelte',
    open: (p) =>
      p
        .getByRole('button', { name: /^Delete – / })
        .first()
        .click(),
  },
  {
    // The only one that needs the application to have done something first:
    // history holds applied decisions, so one has to be applied.
    path: '/history',
    covers: 'pages/History.svelte',
    prepare: async (p) => {
      // Writing is refused while the global dry run is on, which is the
      // shipped default and the reason nothing reached history at first.
      await api('/settings', {
        method: 'PUT',
        body: JSON.stringify({ settings: { global_dry_run: 'false' } }),
      });
      await ready(p, '/simulation');
      await p.getByRole('button', { name: /run simulation/i }).click();
      await p.locator('tbody input[type="checkbox"]').first().check();
      await p.getByRole('button', { name: /apply selected/i }).click();
      await expect(p.locator('.banner-success')).toBeVisible();
    },
    open: (p) =>
      p
        .getByRole('button', { name: /^Revert – / })
        .first()
        .click(),
  },
];

test('every modal names itself and every control inside it', async ({ page, instanceId }) => {
  expect(instanceId).toBeTruthy();

  // The delete confirmation needs something to delete.
  await api('/rules', {
    method: 'POST',
    body: JSON.stringify({
      name: 'Sweepable',
      target_category: 'anime',
      media_type: 'movie',
      priority: 90,
      enabled: true,
      condition_logic: 'any',
      conditions: [{ type: 'title_contains', value: ['akira'] }],
      exclusions: [],
    }),
  });

  const problems: string[] = [];

  for (const [index, modal] of MODALS.entries()) {
    await modal.prepare?.(page);
    await ready(page, modal.path);
    await modal.open(page);

    const dialog = page.locator('dialog[open]');
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
  }

  expect(problems).toEqual([]);
});
