import type { Locator } from '@playwright/test';

import { test, expect, api, openScreen, proveWithKey, unfold } from './fixtures';
import { seedRows } from './seed';
import { SCREENS as ROUTES } from './screens';

/**
 * Layout facts that only a real browser can establish. jsdom computes no
 * geometry, so the component suite can assert that markup exists but never that
 * it fits.
 */

/**
 * How far apart two things can be vertically and still read as one line. Not a
 * cell height: a `td` is as tall as the tallest cell in its row, so measuring it
 * says more about the Actions column than about this one.
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

test.describe('a long title or path is read whole', () => {
  /**
   * A path cut short shows a keyboard and a finger only part of it, and
   * sibling folders differ at their end: `/mnt/storage/media/movies` and
   * `/mnt/storage/media/movies-anime` must not read as one.
   */
  test('a path too long for its cell wraps, and shows all of it', async ({ page }) => {
    await page.goto('/library');

    const cell = page.locator('.cell-path').first();
    await expect(cell).toBeVisible();
    const shown = await cell.evaluate((node) => {
      const holder = node.querySelector('bdi') ?? node;
      holder.textContent = '/mnt/storage/media/library/movies-anime/extended';
      const range = document.createRange();
      range.selectNodeContents(node);
      const lines = new Set([...range.getClientRects()].map((rect) => Math.round(rect.top)));
      return { hidden: node.scrollWidth > node.clientWidth, lines: lines.size };
    });
    expect(shown.hidden).toBe(false);
    expect(shown.lines).toBeGreaterThan(1);
  });

  /**
   * Wrapped at a fixed width instead, a title or a path breaks with half its
   * column empty beside it.
   */
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

  /**
   * A bare title shrinks its column to the longest word and stacks a long one
   * a word per line, pushing the reasons past the edge of the table.
   */
  test("a proposal's long title wraps within its column, not a word per line", async ({ page }) => {
    await api('/rules', {
      method: 'POST',
      body: JSON.stringify({
        name: 'Totoro',
        target_category: 'anime',
        media_type: 'movie',
        priority: 10,
        enabled: true,
        condition_logic: 'any',
        conditions: [{ type: 'title_contains', value: ['totoro'] }],
        exclusions: [],
      }),
    });
    await openScreen(page, '/simulation');
    await page.getByRole('button', { name: /run simulation/i }).click();

    const title = page.locator('td .cell-title', { hasText: 'My Neighbor Totoro' });
    await expect(title).toBeVisible();
    // Polled: the run redraws the table, and a row measured mid-redraw has no
    // line at all.
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
 * A phone. Below the drawer breakpoint the navigation leaves the flow, and a
 * table has to scroll inside its own region rather than drag the page with it.
 * Nothing in the unit tests computes layout, so this is where a 375px screen is
 * seen at all.
 */
test.describe('on a phone', () => {
  for (const path of ROUTES) {
    test(`${path} fits a 375px screen and its tables scroll inside their region`, async ({
      page,
    }) => {
      // With a row in every table, once loaded: a heading over an empty table
      // fits any screen.
      await seedRows();
      await page.setViewportSize({ width: 375, height: 812 });
      await openScreen(page, path);
      await page.waitForLoadState('networkidle');
      await unfold(page);

      const overflow = await page.evaluate(
        () => document.documentElement.scrollWidth - document.documentElement.clientWidth,
      );
      expect(overflow, `${path} scrolls the page sideways by ${overflow}px`).toBeLessThanOrEqual(1);

      // A table wider than the screen is expected. The region around it is what
      // scrolls, and it has to be reachable to do so.
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

  /**
   * A section the sweep above cannot reach.
   *
   * `/settings` opens on its first tab, so the source list is drawn narrow
   * only here. Held in three lanes, the actions keep the width that aligns
   * their right edges, which squeezes the credential field beside them below
   * the word it holds and clips a button outside its own row. The document
   * does not scroll when that happens, so the check above passes, and nothing
   * but a measurement inside the row shows it.
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
   * The same screens at 360px with every counter set at once.
   *
   * The fixture alone never reaches the worst case: the bar's width is a
   * function of how many counts are non-zero *and* of how long the language
   * writes them, so the status is forced here. A fixture that cannot produce
   * the failure is a test that cannot find it.
   */
  test('every screen fits 360px with every counter at once', async ({ page }) => {
    await page.route('**/api/v1/status', async (route) => {
      const response = await route.fetch();
      const body = await response.json();
      await route.fulfill({
        json: {
          ...body,
          dry_run: false,
          running_jobs: 2,
          pending_decisions: 12,
          failed_decisions: 3,
          warnings: ['a', 'b', 'c', 'd'].map((message) => ({ message, guide_step: null })),
        },
      });
    });
    await page.setViewportSize({ width: 360, height: 780 });

    for (const path of ROUTES) {
      await openScreen(page, path);

      const measured = await page.evaluate(() => {
        const bar = document.querySelector('.topbar');
        return {
          overflow: document.documentElement.scrollWidth - document.documentElement.clientWidth,
          // Nothing in the bar may be taller than the bar: a wrapped label is
          // what bursts the chrome.
          tallest: Math.max(
            0,
            ...[...(bar?.querySelectorAll('*') ?? [])].map(
              (el) => el.getBoundingClientRect().height,
            ),
          ),
          barHeight: bar?.getBoundingClientRect().height ?? 0,
        };
      });

      expect(
        measured.overflow,
        `${path} scrolls sideways by ${measured.overflow}px`,
      ).toBeLessThanOrEqual(1);
      expect(
        measured.tallest,
        `${path} has chrome ${measured.tallest}px tall in a ${measured.barHeight}px bar`,
      ).toBeLessThanOrEqual(measured.barHeight);
    }
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

    const token = page.locator('.secret-once');
    await expect(token).toBeVisible();
    const fits = await token.evaluate((node) => {
      const box = node.getBoundingClientRect();
      return {
        page: document.documentElement.scrollWidth - document.documentElement.clientWidth,
        inside: box.right <= document.documentElement.clientWidth,
      };
    });
    expect(fits).toEqual({ page: 0, inside: true });
    const keys = (await api('/applications')) as { id: string }[];
    for (const key of keys) await api(`/applications/${key.id}`, { method: 'DELETE' });
  });

  /** An empty table spans past the screen, and its message, centred across it, starts out of view. */
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

  /** A caption held at 190px beside the picker pushes the picker and Delete past a phone's dialog. */
  test('a condition in the rule editor fits the dialog', async ({ page }) => {
    await page.setViewportSize({ width: 375, height: 812 });
    await openScreen(page, '/rules');
    await page.getByRole('button', { name: 'New Rule' }).click();
    const list = 'Conditions (all of)';
    await page.getByRole('combobox', { name: list, exact: true }).selectOption('genre_contains');
    await page.getByRole('button', { name: `Add – ${list}` }).click();

    const row = page.locator('.condition-row').first();
    await expect(row).toBeVisible();
    // Room for the value as well: squeezed to nothing beside the caption, the
    // picker fits the dialog and cannot be used.
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

    // In the viewport rather than visible: a closed drawer is only translated
    // off the screen, which still counts as visible.
    const link = sidebar.locator('a[href="/library"]');
    await expect(link).not.toBeInViewport();
    await page.getByRole('button', { name: 'Open navigation' }).click();
    await expect(sidebar).toHaveClass(/is-open/);
    await expect(link).toBeInViewport();

    // Following a link closes it, so the page is not left under a scrim. By
    // href rather than by name: the label is translated and renameable, the
    // destination is not.
    await link.click();
    await expect(sidebar).not.toHaveClass(/is-open/);
    await expect(page).toHaveURL(/\/library/);
  });

  /**
   * The open drawer makes the page inert, its toggle included, and a screen
   * reader's swipe sends no Escape: the drawer carries its own way out, which
   * hands the focus back to the toggle.
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
   * Without a fixed height the same `btn btn-primary` renders at three sizes:
   * a flex row stretches buttons to their tallest sibling, `<label class="btn">`
   * inherits a line-height `<button>` resets, and a text label makes a taller
   * line box than an icon. All three are invisible to a unit test.
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

    // And the two sizes are actually distinct, so this cannot pass by making
    // everything the same size by accident.
    const [regularHeight = 0] = regular.keys();
    const [smallHeight = Infinity] = small.keys();
    expect(regularHeight).toBeGreaterThan(smallHeight);
  });
});

/**
 * `select.form-select` draws its arrow over its end padding. A rule that sets
 * the padding in one shorthand, as a compact row does, draws the arrow over
 * the text, and "all of" reads as a glyph sitting on its last letter.
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

  // The compact selects live in the rule editor, one per condition that has a
  // quantifier to choose.
  await page.goto('/rules?new=1');
  const dialog = page.getByRole('dialog');
  await dialog
    .getByRole('combobox', { name: 'Conditions (all of)', exact: true })
    .selectOption('genre_contains');
  await dialog.getByRole('button', { name: 'Add – Conditions (all of)' }).click();
  await expect(dialog.getByRole('combobox', { name: /^How these values combine/ })).toBeVisible();
  await measure('rule editor');

  expect(cramped).toEqual([]);
});

test.describe('the chrome draws one line', () => {
  /**
   * The sidebar header and the top bar sit side by side and each ends in a
   * rule, so together they read as one line across the window, unless their
   * heights differ, which puts a step in the middle of it. Both take
   * `--chrome-height`, and only a browser can measure where a border actually
   * lands.
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
   * A dot inside a framed chip is read against the frame, not against the
   * letters beside it, so it sits on the frame's middle. The kind badge draws
   * its dot as `::before`, which no query can measure, so what is checked
   * there is whatever could still move it once the row has centred it.
   */
  test('a dot sits on the middle of the chip that frames it', async ({ page }) => {
    await openScreen(page, '/instances');

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
 * Ticking a box may not move the page under the pointer.
 *
 * The checkbox is an `inline-grid`, and its tick only exists when it is
 * checked: with no in-flow child the box synthesises its baseline from the
 * bottom of its margin box, so the two states would make line boxes of
 * different heights, and selecting all would lift every row below the head
 * under the very pointer that had just clicked. Only a head shows it, a body
 * row being as tall as its text, and no DOM query can see it at all.
 */
test('selecting rows does not move the rows', async ({ page }) => {
  // A rule this library actually matches: without one the run proposes nothing
  // and the table has no rows to select, so the check would pass by measuring
  // an empty page.
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

  await page.goto('/simulation');
  await page.getByRole('button', { name: /run simulation/i }).click();
  // The run is over once its moves arrive ticked. The rows the screen opened
  // on, left pending by an earlier run, are visible before it.
  await expect(page.getByRole('checkbox', { name: 'Select the move for "Akira"' })).toBeChecked();

  const selectAll = page.getByRole('checkbox', { name: 'Select every proposed move' });
  const firstRow = page.locator('tbody tr').first();

  // The cell holding the box and nothing else. Measuring the whole head hides
  // the defect wherever a longer heading is the taller thing in that row,
  // which is most languages, and this fixture.
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

/**
 * A table that does not scroll shows no scroll shadow.
 *
 * The shadows are painted as four background layers on the scroll container:
 * two "covers" pinned to the content (`background-attachment: local`) and two
 * shadows pinned to the frame (`scroll`), so each shadow is hidden until there
 * is something in that direction. It only works if the cover is *opaque* across
 * the width of the shadow it hides: a cover fading from its first pixel lets
 * the shadow through, and every table with nothing to scroll carries a dark
 * band down both edges.
 *
 * Measured rather than eyeballed: nothing about this is visible to a DOM
 * query, and it reads as a design choice until you sample the pixels.
 */
test('a table with nothing to scroll has no shadow down its edges', async ({ page }) => {
  // In the light theme: on the dark card a shadow darkens the edge by a unit,
  // which one engine's rounding also does, and on the light one by tens.
  await api('/settings', {
    method: 'PUT',
    body: JSON.stringify({ settings: { ui_theme: 'light' } }),
  });
  await openScreen(page, '/move-log');

  const container = page.locator('.table-container').first();
  await container.waitFor({ state: 'visible' });

  const overflows = await container.evaluate((el) => el.scrollWidth > el.clientWidth);
  expect(overflows, 'the viewport is too narrow for this test to mean anything').toBe(false);
  // A pointer left over a row by an earlier test paints that row's hover fill
  // into the sampled strip, whatever the cover does.
  await page.mouse.move(0, 0);

  // A row, not the header: `th` paints its own opaque background and would hide
  // the container's regardless.
  const shot = await container.screenshot();
  const edges = await page.evaluate(async (bytes) => {
    const blob = new Blob([new Uint8Array(bytes)], { type: 'image/png' });
    const bitmap = await createImageBitmap(blob);
    const canvas = new OffscreenCanvas(bitmap.width, bitmap.height);
    const ctx = canvas.getContext('2d')!;
    ctx.drawImage(bitmap, 0, 0);
    // Below the header row, wherever the first data row happens to fall.
    const y = Math.min(bitmap.height - 1, 130);
    const strip = (x: number) => Array.from(ctx.getImageData(x, y, 16, 1).data);
    // Two pixels in: WebKit draws the container's edge one pixel wider.
    return { left: strip(2), right: strip(bitmap.width - 18), width: bitmap.width };
  }, Array.from(shot));

  // Each 16px strip must be one flat colour: a gradient means a shadow, three
  // units deep at least here. One unit per channel is an engine's rounding.
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

/**
 * One look for one thing, measured where only a browser can: while a screen
 * loads, on a phone, and across every screen of the route table.
 */
test.describe('every screen draws a shared thing the same way', () => {
  /** A screen whose title waits for its data reads as a broken page. */
  test('every screen keeps its title while its data loads', async ({ page }) => {
    // Held rather than failed, so each screen stays in its loading state. The
    // dictionary and the sign-in mode are what any screen needs to draw at all.
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

  /**
   * A value Save waits on is marked on its field, in words under it and on its
   * tab, since from another tab the field is out of sight. The border and the
   * mark are what only a stylesheet draws.
   */
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
