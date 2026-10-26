import type { Locator, Page } from '@playwright/test';

import { test, expect, api, openScreen, proveWithKey, unfold } from './fixtures';
import { seedRows } from './seed';
import { SCREENS as ROUTES } from './screens';
import type { RuleDraft } from '../src/api/types';

/**
 * How far apart two things can be vertically and still read as one line. Measured on a cell's
 * children: a `td` is as tall as the tallest cell in its row.
 */
const SAME_LINE = 12;

/** Vertical distance between the centres of a cell's children. */
async function verticalSpread(cell: Locator): Promise<number> {
  return cell.evaluate((node) => {
    const centres = [...node.children].map((child) => {
      const box = child.getBoundingClientRect();
      return box.top + box.height / 2;
    });
    return centres.length < 2 ? 0 : Math.max(...centres) - Math.min(...centres);
  });
}

/** A new rule in the editor, given a genre condition through the editor's own menu. */
async function addGenreCondition(page: Page): Promise<Locator> {
  await openScreen(page, '/rules');
  await page.getByRole('button', { name: 'New Rule' }).click();
  await page.getByRole('button', { name: 'Add a condition – Conditions', exact: true }).click();
  await page.getByRole('menuitem', { name: /^Genre contains/ }).click();
  const row = page.locator('.condition-row').first();
  await expect(row).toBeVisible();
  return row;
}

test.describe('a long title or path is read whole', () => {
  /**
   * A path cut short hides its end, where siblings differ: `/mnt/storage/media/movies` and
   * `/mnt/storage/media/movies-anime` must not read as one.
   */
  test('a path too long for its cell wraps, and shows all of it', async ({ page }) => {
    await page.goto('/library');

    const cell = page.locator('.cell-path').first();
    await expect(cell).toBeVisible();
    const shown = await cell.evaluate((node) => {
      const holder = node.querySelector('bdi') ?? node;
      holder.textContent =
        '/mnt/storage/media/library/movies-anime/extended-editions/the-longest-folder-names-a-library-holds';
      const range = document.createRange();
      range.selectNodeContents(node);
      const lines = new Set([...range.getClientRects()].map((rect) => Math.round(rect.top)));
      return { hidden: node.scrollWidth > node.clientWidth, lines: lines.size };
    });
    expect(shown.hidden).toBe(false);
    expect(shown.lines).toBeGreaterThan(1);
  });

  /** Wrapped at a fixed width, a title or a path would break with half its column empty. */
  test('a title and a path their column has room for stay on one line', async ({ page }) => {
    await page.setViewportSize({ width: 1600, height: 900 });
    await openScreen(page, '/library');
    const linesOf = (cell: Locator, text: string) =>
      cell.evaluate((node, written) => {
        (node.querySelector('bdi') ?? node.firstChild ?? node).textContent = written;
        const range = document.createRange();
        range.selectNodeContents(node);
        return new Set([...range.getClientRects()].map((rect) => Math.round(rect.top))).size;
      }, text);

    const title = page.locator('td .cell-title').first();
    await expect(title).toBeVisible();
    expect(await linesOf(title, 'Billions Club Live with The Weeknd: A Concert Film')).toBe(1);
    expect(
      await linesOf(page.locator('td .cell-path').first(), '/data/media/movies/concerts-and-live'),
    ).toBe(1);
  });

  /** A bare title shrinks its column to its longest word and pushes the reasons off the table. */
  test("a proposal's long title wraps within its column, not a word per line", async ({ page }) => {
    await api('/rules', {
      method: 'POST',
      body: JSON.stringify({
        name: 'Totoro',
        target_category: 'anime',
        media_type: 'movie',
        priority: 10,
        enabled: true,
        conditions: [{ type: 'title_contains', value: ['totoro'] }],
        exclusions: [],
      } satisfies Partial<RuleDraft>),
    });
    await openScreen(page, '/simulation');
    await page.getByRole('button', { name: /run simulation/i }).click();

    const title = page.locator('td .cell-title', { hasText: 'My Neighbor Totoro' });
    await expect(title).toBeVisible();
    // Polled: a row measured while the run redraws the table has no line at all.
    await expect
      .poll(() =>
        title.evaluate((node) => {
          node.textContent = 'My Neighbor Totoro: The Spirits of the Forest, Collected';
          const range = document.createRange();
          range.selectNodeContents(node);
          const lines = new Set([...range.getClientRects()].map((rect) => Math.round(rect.top)));
          return node.scrollWidth <= node.clientWidth && lines.size >= 2 && lines.size <= 3;
        }),
      )
      .toBe(true);
  });
});

/** The reasons are what a proposal is checked against, and nobody reads past the edge. */
test('the proposals fit a 1440 pixel screen without scrolling sideways', async ({ page }) => {
  await page.setViewportSize({ width: 1440, height: 900 });
  await api('/rules', {
    method: 'POST',
    body: JSON.stringify({
      name: 'Japanese animation',
      target_category: 'anime',
      media_type: 'movie',
      priority: 10,
      enabled: true,
      conditions: [{ type: 'title_contains', value: ['akira', 'totoro'] }],
      exclusions: [],
    } satisfies Partial<RuleDraft>),
  });
  await openScreen(page, '/simulation');
  await page.getByRole('button', { name: /run simulation/i }).click();
  await expect(page.getByRole('checkbox', { name: 'Select the move for "Akira"' })).toBeChecked();

  const proposals = page.locator('.table-container').last();
  const overflow = await proposals.evaluate((box) => box.scrollWidth - box.clientWidth);
  expect(overflow, `the proposals run ${overflow}px past the edge`).toBeLessThanOrEqual(0);
});

test.describe('table cells stay on one line', () => {
  test('a timestamp is written the way the language writes it', async ({ page }) => {
    await page.goto('/instances');

    const cell = page.locator('td.cell-timestamp').first();
    // Not the stored form: that is a log line, not a date.
    await expect(cell).not.toHaveText(/\d{4}-\d{2}-\d{2} \d{2}:\d{2}:\d{2}/);
    // The exact value stays reachable for an audit, on the title attribute.
    await expect(cell).toHaveAttribute('title', /\d{4}-\d{2}-\d{2}/);
  });

  /** Greek and German run long: if any language folds the cell it is one of them. */
  test('the last-sync column keeps its date and status on one line', async ({ page }) => {
    await api('/settings', {
      method: 'PUT',
      body: JSON.stringify({ settings: { ui_language: 'de' } }),
    });
    await page.goto('/instances');

    const cell = page.locator('td.cell-timestamp').first();
    await expect(cell).toBeVisible();
    // Both halves present, or a cell holding the date alone would pass.
    await expect(cell.locator('.badge')).toBeVisible();
    expect(
      await verticalSpread(cell),
      'the timestamp and its status badge must share a line, not stack',
    ).toBeLessThan(SAME_LINE);
  });
});

/**
 * A phone, below the drawer breakpoint: the navigation leaves the flow and a table scrolls
 * inside its own region.
 */
test.describe('on a phone', () => {
  // 320 is the width WCAG 1.4.10 reflows to, a 1280px window zoomed to 400%.
  for (const width of [375, 320]) {
    for (const path of ROUTES) {
      test(`${path} fits a ${width}px screen and its tables scroll inside their region`, async ({
        page,
      }) => {
        // A row in every table, once loaded, since a heading over an empty table fits any screen.
        await seedRows();
        await page.setViewportSize({ width, height: 812 });
        await openScreen(page, path);
        await page.waitForLoadState('networkidle');
        await unfold(page);

        const overflow = await page.evaluate(
          () => document.documentElement.scrollWidth - document.documentElement.clientWidth,
        );
        expect(overflow, `${path} scrolls the page sideways by ${overflow}px`).toBeLessThanOrEqual(
          1,
        );

        // A table may be wider than the screen, so the region that scrolls it must be reachable.
        const regions = await page.evaluate(() =>
          [...document.querySelectorAll('.table-container')].map((el) => ({
            scrolls: el.scrollWidth > el.clientWidth,
            focusable: el.getAttribute('tabindex') === '0',
          })),
        );
        for (const region of regions.filter((r) => r.scrolls)) {
          expect(region.focusable).toBe(true);
        }
      });
    }
  }

  for (const [path, answer] of [
    ['/library', 'td:nth-child(2)'],
    ['/move-log', 'td:nth-child(2)'],
    ['/tasks', 'td:nth-child(2)'],
    ['/simulation', 'td:nth-child(5)'],
    ['/history', 'td:nth-child(4)'],
  ] as const) {
    test(`${path} shows what it answers on a phone with nothing to scroll sideways`, async ({
      page,
    }) => {
      await seedRows();
      await page.setViewportSize({ width: 375, height: 812 });
      await openScreen(page, path);

      const cell = page.locator(`main tbody tr ${answer}`).first();
      await expect(cell).toBeVisible();
      const measured = await cell.evaluate((node) => {
        const region = node.closest('.table-container')!;
        const [box, frame] = [node, region].map((el) => el.getBoundingClientRect());
        return {
          scrolls: region.scrollWidth - region.clientWidth,
          inside: box!.left >= frame!.left - 1 && box!.right <= frame!.right + 1,
        };
      });
      expect(measured.scrolls, `${path} scrolls its table sideways`).toBeLessThanOrEqual(0);
      expect(measured.inside, `${path} draws its answer out of sight`).toBe(true);
    });
  }

  /** The folder table may scroll on a phone, its group controls held in its visible part. */
  test("an instance's sync and destination form stay in sight on a phone", async ({ page }) => {
    await api('/instances', {
      method: 'POST',
      body: JSON.stringify({
        name: 'Radarr UHD',
        instance_type: 'radarr',
        base_url: 'http://127.0.0.1:1',
        api_key: 'e2e',
        enabled: true,
        sync_interval_minutes: 60,
      }),
    });
    await page.setViewportSize({ width: 375, height: 812 });
    await openScreen(page, '/categories');

    for (const control of [
      page.getByRole('button', { name: 'Sync now – Radarr UHD' }),
      page.getByText('Add a destination folder on Radarr', { exact: true }),
    ]) {
      await expect(control).toBeVisible();
      const inside = await control.evaluate((node) => {
        const region = node.closest('.table-container')!;
        const [box, frame] = [node, region].map((el) => el.getBoundingClientRect());
        return box!.left >= frame!.left - 1 && box!.right <= frame!.right + 1;
      });
      expect(inside, `${await control.textContent()} is out of sight`).toBe(true);
    }
  });

  /**
   * `/settings` opens on its first tab, so only here is the source list drawn narrow. Held in
   * three lanes, the actions squeeze the credential field, which the document does not scroll
   * for, so only a measurement inside the row shows it.
   */
  test('the source list stacks rather than squeezing its own field', async ({ page }) => {
    await page.setViewportSize({ width: 360, height: 812 });
    await page.goto('/sources');
    await expect(page.locator('.source-row').first()).toBeVisible();

    const measured = await page.evaluate(() => {
      const spill = [...document.querySelectorAll('.source-row')].map((row) => {
        const edge = row.getBoundingClientRect().right;
        const children = [...row.querySelectorAll('*')].map(
          (el) => el.getBoundingClientRect().right,
        );
        return Math.max(...children, 0) - edge;
      });
      const field = document.querySelector('.source-key input');
      return {
        spill: Math.round(Math.max(...spill)),
        field: field ? Math.round(field.getBoundingClientRect().width) : 0,
      };
    });

    expect(
      measured.spill,
      `a source row spills ${measured.spill}px past its own edge`,
    ).toBeLessThanOrEqual(0);
    // Wide enough to read a pasted key back, not merely present.
    expect(
      measured.field,
      `the credential field is ${measured.field}px wide`,
    ).toBeGreaterThanOrEqual(200);
  });

  /**
   * Every chip is forced on, the longer mode word and the guide included: the fixture alone
   * never reaches the widest bar.
   */
  for (const touch of [false, true]) {
    test.describe(touch ? 'under a finger' : 'under a pointer', () => {
      test.use({ hasTouch: touch });

      test('every screen fits a phone with every chip of the bar at once', async ({
        page,
        browserName,
      }) => {
        test.setTimeout(120_000);
        await api('/onboarding', { method: 'PUT', body: JSON.stringify({ state: 'pending' }) });
        await page.route('**/api/v1/status', async (route) => {
          const response = await route.fetch();
          const body = await response.json();
          await route.fulfill({
            json: {
              ...body,
              dry_run: true,
              running_jobs: 2,
              pending_decisions: 12,
              failed_decisions: 3,
              warnings: ['a', 'b', 'c', 'd'].map((message) => ({ message, guide_step: null })),
            },
          });
        });

        const check = async (where: string) => {
          await expect(page.locator('.guide-pill')).toBeVisible();
          const measured = await page.evaluate(() => {
            const bar = document.querySelector('.topbar');
            const height = (el: Element) => el.getBoundingClientRect().height;
            // A chip's target is the transparent box drawn around it.
            const target = (el: Element) => {
              const own = el.getBoundingClientRect();
              const around = getComputedStyle(el, '::before');
              return Math.min(
                Math.max(own.width, parseFloat(around.width) || 0),
                Math.max(own.height, parseFloat(around.height) || 0),
              );
            };
            return {
              overflow: document.documentElement.scrollWidth - document.documentElement.clientWidth,
              // A label that wraps bursts the chrome, so nothing may be taller than the bar.
              tallest: Math.max(0, ...[...(bar?.querySelectorAll('*') ?? [])].map(height)),
              barHeight: bar ? height(bar) : 0,
              chips: new Set(
                [...(bar?.querySelectorAll('.mode-chip, .attention, .guide-pill') ?? [])].map(
                  (el) => Math.round(height(el)),
                ),
              ).size,
              coarse: matchMedia('(pointer: coarse)').matches,
              smallest: Math.min(...[...(bar?.querySelectorAll('a, button') ?? [])].map(target)),
            };
          });

          expect(measured.overflow, `${where} scrolls sideways`).toBeLessThanOrEqual(1);
          expect(
            measured.tallest,
            `${where} has chrome ${measured.tallest}px tall in a ${measured.barHeight}px bar`,
          ).toBeLessThanOrEqual(measured.barHeight);
          expect(measured.chips, `${where} draws its chips at several heights`).toBe(1);
          // Chromium emulates touch as a coarse pointer: there the check below must run.
          if (touch && browserName === 'chromium') expect(measured.coarse).toBe(true);
          if (touch && measured.coarse) {
            expect(
              measured.smallest,
              `${where} offers a target ${measured.smallest}px across`,
            ).toBeGreaterThanOrEqual(44);
          }
        };

        for (const width of [375, 320]) {
          await page.setViewportSize({ width, height: 780 });
          for (const path of ROUTES) {
            await openScreen(page, path);
            await check(`${path} at ${width}px`);
          }
        }
        // Spanish and Russian write the dry-run mode in the longest words.
        for (const language of ['es', 'ru']) {
          await api('/settings', {
            method: 'PUT',
            body: JSON.stringify({ settings: { ui_language: language } }),
          });
          await openScreen(page, '/');
          await check(`the dashboard in ${language} at 320px`);
        }
      });
    });
  }

  test('every settings tab shows on a phone, in German too', async ({ page }) => {
    await api('/settings', {
      method: 'PUT',
      body: JSON.stringify({ settings: { ui_language: 'de' } }),
    });
    await page.setViewportSize({ width: 375, height: 812 });
    await openScreen(page, '/settings');

    const tabs = page.getByRole('tab');
    await expect(tabs).toHaveCount(6);
    for (const tab of await tabs.all()) await expect(tab).toBeInViewport({ ratio: 1 });
  });

  /** The only readable copy of a credential, and an 85-character token has no break point. */
  test('a key shown once wraps inside the page', async ({ page }) => {
    await page.setViewportSize({ width: 375, height: 812 });
    await openScreen(page, '/applications');
    await page.getByRole('button', { name: 'New key' }).click();
    const dialog = page.getByRole('dialog');
    await dialog.getByLabel('Name').fill('phone');
    await dialog.getByRole('button', { name: 'Create a key' }).click();
    await proveWithKey(page);

    const copy = page.getByRole('button', { name: 'Copy – Key for phone' });
    await expect(copy).toBeInViewport();
    const fits = await copy.evaluate((node) => {
      const box = node.previousElementSibling!.getBoundingClientRect();
      return {
        page: document.documentElement.scrollWidth - document.documentElement.clientWidth,
        inside: box.right <= document.documentElement.clientWidth,
      };
    });
    expect(fits).toEqual({ page: 0, inside: true });
    const keys = (await api('/applications')) as { id: string }[];
    for (const key of keys) await api(`/applications/${key.id}`, { method: 'DELETE' });
  });

  /** Each webhook value is pasted into the Arr whole, so none may end at the dialog's edge. */
  test("the instance dialog's webhook values wrap inside it, each beside its Copy", async ({
    page,
  }) => {
    await page.setViewportSize({ width: 375, height: 812 });
    await openScreen(page, '/instances');
    await page.getByRole('button', { name: 'Edit – Radarr' }).click();
    const dialog = page.getByRole('dialog', { name: 'Edit instance' });
    await unfold(page);

    for (const name of [
      'Webhook URL',
      'Token for the X-Routarr-Token header',
      'Webhook URL for an older Arr',
    ]) {
      const copy = dialog.getByRole('button', { name: `Copy – ${name}` });
      await copy.scrollIntoViewIfNeeded();
      const fits = await copy.evaluate((node) => {
        const frame = node.closest('[role="dialog"]')!.getBoundingClientRect();
        const value = node.previousElementSibling!;
        const box = value.getBoundingClientRect();
        return {
          cut: value.scrollWidth - value.clientWidth,
          inside: box.left >= frame.left && node.getBoundingClientRect().right <= frame.right,
        };
      });
      expect(fits, `${name} runs past the dialog`).toEqual({ cut: 0, inside: true });
    }
  });

  /** An empty table spans past the screen, so a message centred across it starts out of view. */
  test("an empty table's message sits inside the visible part of its region", async ({ page }) => {
    await page.setViewportSize({ width: 375, height: 812 });
    await openScreen(page, '/exceptions');

    const message = page.locator('td > .empty-state');
    await expect(message).toBeVisible();
    const placed = await message.evaluate((node) => {
      const box = node.getBoundingClientRect();
      const region = node.closest('.table-container')!.getBoundingClientRect();
      return box.left >= region.left - 1 && box.right <= region.right + 1;
    });
    expect(placed).toBe(true);
  });

  /** A caption held at 190px pushes the picker and Delete past a phone's dialog. */
  test('a condition in the rule editor fits the dialog', async ({ page }) => {
    await page.setViewportSize({ width: 375, height: 812 });
    const row = await addGenreCondition(page);
    // Room for the value too: squeezed beside the caption, the picker fits and cannot be used.
    const fits = await row.evaluate((node) => {
      const dialog = node.closest('dialog')!;
      const remove = node.querySelector('button[id$="-delete"]')!.getBoundingClientRect();
      const value = node.querySelector('.condition-value')!.getBoundingClientRect();
      return {
        scrolls: dialog.scrollWidth > dialog.clientWidth,
        deleteInside: remove.right <= dialog.getBoundingClientRect().right,
        valueUsable: value.width >= 120,
      };
    });
    expect(fits).toEqual({ scrolls: false, deleteInside: true, valueUsable: true });
  });

  test('the navigation is a drawer that opens and closes', async ({ page }) => {
    await page.setViewportSize({ width: 375, height: 812 });
    await page.goto('/rules');

    const sidebar = page.locator('.sidebar');
    await expect(sidebar).not.toHaveClass(/is-open/);

    // In the viewport, since a closed drawer is only translated off screen and still visible.
    const link = sidebar.locator('a[href="/library"]');
    await expect(link).not.toBeInViewport();
    await page.getByRole('button', { name: 'Open navigation' }).click();
    await expect(sidebar).toHaveClass(/is-open/);
    await expect(link).toBeInViewport();

    // Following a link closes the drawer. Found by href, which no translation renames.
    await link.click();
    await expect(sidebar).not.toHaveClass(/is-open/);
    await expect(page).toHaveURL(/\/library/);
  });

  /**
   * The open drawer makes the page inert, its toggle included, and a screen reader's swipe
   * sends no Escape, so the drawer carries its own way out.
   */
  test('the open drawer closes from a button of its own', async ({ page }) => {
    await page.setViewportSize({ width: 375, height: 812 });
    await openScreen(page, '/rules');
    const toggle = page.getByRole('button', { name: 'Open navigation' });
    await toggle.click();
    const sidebar = page.locator('.sidebar');
    await expect(sidebar).toHaveClass(/is-open/);

    // Back from the first link, past the scrolling list Firefox stops at.
    const close = sidebar.getByRole('button', { name: 'Dismiss' });
    for (let press = 0; press < 3; press += 1) {
      if (await close.evaluate((button) => button === document.activeElement)) break;
      await page.keyboard.press('Shift+Tab');
    }
    await expect(close).toBeFocused();
    await page.keyboard.press('Enter');

    await expect(sidebar).not.toHaveClass(/is-open/);
    await expect(toggle).toBeFocused();

    // Escape, the keyboard's way out, hands the focus back the same way.
    await toggle.click();
    await expect(sidebar).toHaveClass(/is-open/);
    await page.keyboard.press('Escape');
    await expect(sidebar).not.toHaveClass(/is-open/);
    await expect(toggle).toBeFocused();
  });
});

test.describe('spacing the reset takes away', () => {
  /** In a narrow column, bare badges stack and touch. */
  test("a key's scope badges keep apart however they wrap", async ({ page }) => {
    await api('/applications', {
      method: 'POST',
      body: JSON.stringify({ name: 'Every scope', scopes: ['operate', 'write'], may_confirm: [] }),
    });
    await page.setViewportSize({ width: 375, height: 812 });
    await openScreen(page, '/applications');

    const row = page.getByRole('row', { name: /Every scope/ });
    const gaps = await row.locator('.badge').evaluateAll((badges) => {
      const boxes = badges.map((badge) => badge.getBoundingClientRect());
      return boxes.slice(1).map((box, i) => {
        const before = boxes[i]!;
        const sameLine = Math.abs(box.top - before.top) < 4;
        return sameLine ? box.left - before.right : box.top - before.bottom;
      });
    });
    expect(gaps).toHaveLength(2);
    for (const gap of gaps) expect(gap).toBeGreaterThanOrEqual(4);
    const keys = (await api('/applications')) as { id: string }[];
    for (const key of keys) await api(`/applications/${key.id}`, { method: 'DELETE' });
  });
});

test.describe('buttons are one size', () => {
  /**
   * Without a fixed height one `btn` renders at three sizes: stretched in a flex row, as a
   * `<label>` keeping its line-height, or with a text label's taller line box.
   */
  test('every button on every page shares its size', async ({ page }) => {
    const regular = new Map<number, string>();
    const small = new Map<number, string>();
    // The row actions are buttons too, and an empty table draws none.
    await seedRows();

    for (const path of ROUTES) {
      await openScreen(page, path);
      await page.waitForLoadState('networkidle');

      const found = await page.evaluate(() =>
        [...document.querySelectorAll('.btn')]
          .map((el) => ({
            height: Math.round(el.getBoundingClientRect().height),
            small: el.classList.contains('btn-sm'),
            label: (el.textContent || '').trim().slice(0, 24) || '(icon only)',
          }))
          // A button inside a closed dialog has no box to measure.
          .filter((entry) => entry.height > 0),
      );

      for (const entry of found) {
        (entry.small ? small : regular).set(entry.height, `${path} – ${entry.label}`);
      }
    }

    expect(
      regular.size,
      `regular buttons differ: ${[...regular].map(([h, w]) => `${h}px (${w})`).join(', ')}`,
    ).toBe(1);
    expect(
      small.size,
      `small buttons differ: ${[...small].map(([h, w]) => `${h}px (${w})`).join(', ')}`,
    ).toBe(1);

    // The two sizes stay distinct, so this cannot pass by making everything one size.
    const [regularHeight = 0] = regular.keys();
    const [smallHeight = Infinity] = small.keys();
    expect(regularHeight).toBeGreaterThan(smallHeight);
  });
});

/** The value is what a person types and checks, beside a caption chosen once. */
test("a condition's value takes the room its row leaves", async ({ page }) => {
  await page.setViewportSize({ width: 1280, height: 800 });
  const row = await addGenreCondition(page);

  const share = await row.evaluate(
    (node) =>
      node.querySelector('.condition-value')!.getBoundingClientRect().width /
      node.getBoundingClientRect().width,
  );
  expect(share).toBeGreaterThanOrEqual(0.4);
});

/**
 * `select.form-select` draws its arrow over its end padding, so a padding shorthand, as in a
 * compact row, puts the arrow over the text.
 */
test('every select keeps the room its arrow is drawn in', async ({ page }) => {
  // Every screen in one test: a cold browser on a CI runner needs more than the default.
  test.setTimeout(60_000);
  const cramped: string[] = [];
  const measure = async (where: string) => {
    const found = await page.locator('select.form-select').evaluateAll((selects) =>
      selects
        .filter((el) => el.getBoundingClientRect().width > 0)
        .filter((el) => parseFloat(getComputedStyle(el).paddingInlineEnd) < 28)
        .map((el) => el.getAttribute('aria-label') ?? el.id),
    );
    cramped.push(...found.map((name) => `${where}: ${name}`));
  };

  for (const path of ROUTES) {
    await openScreen(page, path);
    await measure(path);
  }

  // The compact selects live in the rule editor, one per condition with a quantifier.
  await addGenreCondition(page);
  await expect(
    page.getByRole('dialog').getByRole('combobox', { name: /^How these values combine/ }),
  ).toBeVisible();
  await measure('rule editor');

  expect(cramped).toEqual([]);
});

test('the whole navigation shows in a laptop window', async ({ page }) => {
  for (const [width, height] of [
    [1366, 768],
    [1440, 900],
  ] as const) {
    await page.setViewportSize({ width, height });
    await openScreen(page, '/');
    const nav = page.getByRole('navigation', { name: 'Main navigation' });
    const [box, last] = [await nav.boundingBox(), await nav.getByRole('link').last().boundingBox()];
    const cut = last!.y + last!.height - (box!.y + box!.height);
    expect(
      cut,
      `at ${width} by ${height} the last entry ends ${cut}px below the fold`,
    ).toBeLessThanOrEqual(0);
  }
});

test.describe('the chrome draws one line', () => {
  /**
   * The sidebar header and the top bar each end in a rule, read as one line across the window
   * unless their heights differ. Both take `--chrome-height`.
   */
  test('the sidebar header and the top bar end at the same height', async ({ page }) => {
    await openScreen(page, '/library');

    const [sidebar, topbar] = await Promise.all([
      page.locator('.sidebar-header').boundingBox(),
      page.locator('.topbar').boundingBox(),
    ]);

    const sidebarBottom = sidebar!.y + sidebar!.height;
    const topbarBottom = topbar!.y + topbar!.height;
    expect(
      Math.abs(sidebarBottom - topbarBottom),
      `the chrome is stepped: sidebar ends at ${sidebarBottom}, top bar at ${topbarBottom}`,
    ).toBeLessThanOrEqual(1);
  });

  /**
   * A dot in a framed chip is read against the frame, so it sits on the frame's middle. The
   * kind badge's `::before` dot cannot be measured, so what could still move it is checked.
   */
  test('a dot sits on the middle of the chip that frames it', async ({ page }) => {
    await openScreen(page, '/');

    const modeOffset = await page.locator('.mode-chip').evaluate((chip) => {
      const frame = chip.getBoundingClientRect();
      const dot = chip.querySelector('.mode-dot')!.getBoundingClientRect();
      return dot.top + dot.height / 2 - (frame.top + frame.height / 2);
    });
    expect(Math.abs(modeOffset), 'the mode dot is off its chip').toBeLessThanOrEqual(0.5);

    const kindOffset = await page
      .locator('.badge-kind')
      .first()
      .evaluate((badge) => {
        const dot = getComputedStyle(badge, '::before');
        const shift = new DOMMatrix(dot.transform).m42;
        return shift + parseFloat(dot.marginTop) - parseFloat(dot.marginBottom);
      });
    expect(kindOffset, 'the kind dot is off its badge').toBe(0);
  });
});

/**
 * The checkbox is an `inline-grid` whose tick exists only when checked, so its synthesised
 * baseline moves the head's line box, and selecting all would lift the rows under the pointer.
 */
test('selecting rows does not move the rows', async ({ page }) => {
  // A rule this library matches, or the run proposes no rows and the check measures nothing.
  await api('/rules', {
    method: 'POST',
    body: JSON.stringify({
      name: 'Everything japanese',
      target_category: 'anime',
      media_type: 'movie',
      priority: 10,
      enabled: true,
      conditions: [{ type: 'title_contains', value: ['akira', 'totoro', 'perfect blue'] }],
      exclusions: [],
    } satisfies Partial<RuleDraft>),
  });

  await page.goto('/simulation');
  await page.getByRole('button', { name: /run simulation/i }).click();
  // Over once its moves arrive ticked: rows pending from an earlier run show before it.
  await expect(page.getByRole('checkbox', { name: 'Select the move for "Akira"' })).toBeChecked();

  const selectAll = page.getByRole('checkbox', { name: 'Select every proposed move' });
  const firstRow = page.locator('tbody tr').first();

  // The box's own cell: across the whole head, a longer heading hides the defect.
  const cell = () =>
    page.evaluate(() => {
      const box = document.querySelector('thead input[type="checkbox"]');
      const th = box?.closest('th');
      return th ? Math.round(th.getBoundingClientRect().height * 100) / 100 : null;
    });

  await selectAll.uncheck();
  const before = { cell: await cell(), row: (await firstRow.boundingBox())?.y };

  await selectAll.check();
  const after = { cell: await cell(), row: (await firstRow.boundingBox())?.y };

  expect(
    after.cell,
    `the box's own cell is ${before.cell}px unchecked and ${after.cell}px checked`,
  ).toBe(before.cell);
  expect(after.row, `the first row moved from ${before.row} to ${after.row}`).toBe(before.row);
});

/** Sticky at the window's foot, the apply bar would cover the last proposal it applies. */
for (const width of [375, 1440]) {
  test(`the apply bar leaves the last proposal in sight at ${width} pixels`, async ({ page }) => {
    await page.setViewportSize({ width, height: 800 });
    await api('/rules', {
      method: 'POST',
      body: JSON.stringify({
        name: 'Everything japanese',
        target_category: 'anime',
        media_type: 'movie',
        priority: 10,
        enabled: true,
        conditions: [{ type: 'title_contains', value: ['akira', 'totoro', 'perfect blue'] }],
        exclusions: [],
      } satisfies Partial<RuleDraft>),
    });
    await openScreen(page, '/simulation');
    await page.getByRole('button', { name: /run simulation/i }).click();
    await expect(page.getByRole('checkbox', { name: 'Select the move for "Akira"' })).toBeChecked();
    const bar = page.locator('.save-bar');
    await expect(bar.getByRole('button', { name: /^Apply selected/ })).toBeVisible();

    await page.evaluate(() => window.scrollTo(0, document.documentElement.scrollHeight));

    const last = await page.locator('main tbody tr').last().boundingBox();
    const over = await bar.boundingBox();
    expect(last && over, 'the last row or the bar has no box').toBeTruthy();
    expect(
      last!.y + last!.height,
      `the last row ends at ${last!.y + last!.height}px, under the bar from ${over!.y}px`,
    ).toBeLessThanOrEqual(over!.y);
    expect(over!.y + over!.height).toBeLessThanOrEqual(800);
  });
}

/**
 * The scroll shadows hide under covers that must be opaque across a shadow's width, or
 * every table with nothing to scroll carries a dark band down both edges.
 */
test('a table with nothing to scroll has no shadow down its edges', async ({ page }) => {
  // In the light theme, where a shadow darkens the edge by tens and not by a rounding's unit.
  await api('/settings', {
    method: 'PUT',
    body: JSON.stringify({ settings: { ui_theme: 'light' } }),
  });
  await openScreen(page, '/move-log');

  const container = page.locator('.table-container').first();
  await container.waitFor({ state: 'visible' });

  const overflows = await container.evaluate((el) => el.scrollWidth > el.clientWidth);
  expect(overflows, 'the viewport is too narrow for this test to mean anything').toBe(false);
  // A pointer left over a row paints its hover fill into the sampled strip.
  await page.mouse.move(0, 0);

  // Across the first data row, since a `th` paints its own opaque background.
  const row = container.locator('tbody tr').first();
  await expect(row).toBeVisible();
  const [rowBox, containerBox] = [await row.boundingBox(), await container.boundingBox()];
  if (rowBox === null || containerBox === null) throw new Error('the table is not laid out');
  const middle = rowBox.y + rowBox.height / 2 - containerBox.y;
  // Within the cells' padding: further in, a badge in the first cell reads as a gradient.
  const padding = await row.evaluate((tr) => {
    const cells = tr.querySelectorAll('td');
    const first = cells[0];
    const last = cells[cells.length - 1];
    if (!first || !last) throw new Error('the row has no cell');
    return [getComputedStyle(first).paddingLeft, getComputedStyle(last).paddingRight].map(
      parseFloat,
    );
  });
  const shot = await container.screenshot();
  const edges = await page.evaluate(
    async ([bytes, middle, [start, end]]) => {
      const blob = new Blob([new Uint8Array(bytes)], { type: 'image/png' });
      const bitmap = await createImageBitmap(blob);
      const canvas = new OffscreenCanvas(bitmap.width, bitmap.height);
      const ctx = canvas.getContext('2d')!;
      ctx.drawImage(bitmap, 0, 0);
      const y = Math.round(middle * devicePixelRatio);
      // Two pixels in: WebKit draws the container's edge one pixel wider.
      const width = (padding: number) => Math.max(1, Math.floor((padding - 4) * devicePixelRatio));
      const strip = (x: number, wide: number) => Array.from(ctx.getImageData(x, y, wide, 1).data);
      const right = width(end ?? 0);
      return { left: strip(2, width(start ?? 0)), right: strip(bitmap.width - 2 - right, right) };
    },
    [Array.from(shot), middle, padding] as const,
  );
  expect(edges.left.length / 4, 'the cells leave no padding to sample').toBeGreaterThanOrEqual(8);

  // A strip must be one flat colour: three units deep is a shadow, one per channel a rounding.
  for (const [side, pixels] of [
    ['left', edges.left],
    ['right', edges.right],
  ] as const) {
    const first = pixels.slice(0, 4);
    const varied = [];
    for (let i = 0; i < pixels.length; i += 4) {
      const px = pixels.slice(i, i + 4);
      if (px.some((value, channel) => Math.abs(value - (first[channel] ?? 0)) > 1)) {
        varied.push(`x=${i / 4} ${px.join()}`);
      }
    }
    expect(varied, `${side} edge is not flat: ${varied.join(' | ')}`).toEqual([]);
  }
});

/** One look for one thing, measured while a screen loads, on a phone and on every screen. */
test.describe('every screen draws a shared thing the same way', () => {
  /** A screen whose title waits for its data reads as a broken page. */
  test('every screen keeps its title while its data loads', async ({ page }) => {
    // Held, so each screen stays loading. The dictionary and sign-in mode let any screen draw.
    await page.route('**/api/v1/**', async (route) => {
      if (/\/api\/v1\/(localization|auth\/mode)/.test(route.request().url())) {
        await route.continue();
        return;
      }
      await new Promise(() => {});
    });
    for (const path of ROUTES) {
      await openScreen(page, path);
    }
  });

  /** The primary colour says what the screen is for, which is one thing. */
  test('every screen offers at most one primary action', async ({ page }) => {
    for (const shown of ['done', 'pending']) {
      await api('/onboarding', { method: 'PUT', body: JSON.stringify({ state: shown }) });
      for (const path of ROUTES) {
        await openScreen(page, path);
        const primary = await page.locator('.btn-primary:visible').count();
        expect(primary, `${path} with the guide ${shown}`).toBeLessThanOrEqual(1);
      }
    }
  });

  /** A row without a rank keeps its name in the wide lane. */
  test('a source name on Diagnostics holds one line on a phone', async ({ page }) => {
    await page.setViewportSize({ width: 390, height: 844 });
    await page.goto('/diagnostics');
    const names = page.locator('.source-row .source-name');
    await expect(names.first()).toBeVisible();

    const wrapped = await names.evaluateAll((nodes) =>
      nodes
        .filter((node) => {
          const element = node as HTMLElement;
          const before = element.getBoundingClientRect().height;
          element.style.whiteSpace = 'nowrap';
          const after = element.getBoundingClientRect().height;
          element.style.whiteSpace = '';
          return before > after + 1;
        })
        .map((node) => node.textContent),
    );
    expect(wrapped).toEqual([]);
  });

  /** Wrapped under Cancel, the actions still end where the footer ends. */
  test('a wrapped dialog footer keeps its actions at the end', async ({ page }) => {
    await page.setViewportSize({ width: 390, height: 844 });
    await page.goto('/instances?add=1');
    const dialog = page.getByRole('dialog');
    const footer = dialog.locator('.dialog-actions');
    const add = footer.getByRole('button', { name: 'Add instance' });
    await expect(add).toBeVisible();

    const [footerBox, addBox] = [await footer.boundingBox(), await add.boundingBox()];
    const gap = footerBox!.x + footerBox!.width - (addBox!.x + addBox!.width);
    expect(Math.abs(gap), `the action ends ${gap}px before the footer`).toBeLessThanOrEqual(1);
  });

  test('a number setting is a short field, its unit beside it', async ({ page }) => {
    await page.goto('/settings#maintenance');
    const field = page.getByLabel('Backup interval', { exact: true });
    const unit = page.locator('#setting-backup_interval_hours-unit');
    await expect(unit).toHaveText('hours');

    const [box, after] = [await field.boundingBox(), await unit.boundingBox()];
    expect(box!.width, 'the field spans the column').toBeLessThan(160);
    expect(after!.x).toBeGreaterThan(box!.x + box!.width);
    expect(Math.abs(after!.y + after!.height / 2 - (box!.y + box!.height / 2))).toBeLessThan(4);
  });

  /** A value Save waits on is marked on its field and on its tab, for a reader on another tab. */
  test('marks a value outside its bounds on its field and on its tab', async ({ page }) => {
    await page.goto('/settings#guardrails');
    const field = page.getByLabel('Batch limit', { exact: true });
    await field.fill('0');

    await expect(page.getByText('Enter a whole number from 1 to 1000.')).toBeVisible();
    const danger = await page.evaluate(() => {
      const probe = document.createElement('span');
      probe.style.color = 'var(--status-danger)';
      document.body.append(probe);
      const colour = getComputedStyle(probe).color;
      probe.remove();
      return colour;
    });
    // Polled: the border fades to its colour.
    await expect
      .poll(() => field.evaluate((node) => getComputedStyle(node).borderTopColor))
      .toBe(danger);

    await page.getByRole('tab', { name: 'General' }).click();
    const tab = page.getByRole('tab', { name: 'Guardrails: a value is outside its bounds' });
    await expect(tab.locator('svg')).toBeVisible();
  });
});

test('every card of the metadata sources shares one width', async ({ page }) => {
  await page.setViewportSize({ width: 1440, height: 900 });
  await openScreen(page, '/sources');
  await expect(page.getByRole('heading', { name: 'Data sources', exact: true })).toBeVisible();

  const edges = await page
    .locator('main .card')
    .evaluateAll((cards) => [
      ...new Set(
        cards
          .filter((card) => !card.parentElement?.closest('.card'))
          .map((card) => Math.round(card.getBoundingClientRect().right)),
      ),
    ]);
  expect(edges, 'the cards end at more than one edge').toHaveLength(1);
});

/**
 * The library card's "?" lies over its summary, which ends on the chevron that folds the card.
 * Drawn over each other, the two read as one glyph.
 */
test("the library card's help stays clear of its chevron", async ({ page }) => {
  await openScreen(page, '/rules');
  const help = page.locator('.fold-wrap:has(#facets-title) > .fold-help');
  await expect(help).toBeVisible();

  const [helpEnd, chevronStart] = await page.evaluate(() => {
    const summary = document.querySelector('.fold-card:has(#facets-title) > summary')!;
    const chevron = getComputedStyle(summary, '::after');
    // A bordered square turned 45 degrees takes its diagonal on the line.
    const side = parseFloat(chevron.width) + 2 * parseFloat(chevron.borderRightWidth);
    const end =
      summary.getBoundingClientRect().right - parseFloat(getComputedStyle(summary).paddingRight);
    const helpBox = document
      .querySelector('.fold-wrap:has(#facets-title) > .fold-help')!
      .getBoundingClientRect();
    return [helpBox.right, end - side * Math.SQRT2];
  });
  expect(helpEnd).toBeLessThanOrEqual(chevronStart);
});
