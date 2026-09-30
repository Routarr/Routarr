import type { Locator } from '@playwright/test';

import { test, expect, api, openScreen } from './fixtures';
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

test.describe('table cells stay on one line', () => {
  test('the last-sync column does not stack its date and status', async ({ page }) => {
    await page.goto('/instances');

    const cell = page.locator('td.cell-timestamp').first();
    await expect(cell).toBeVisible();
    await expect(cell.locator('.badge')).toBeVisible();

    expect(
      await verticalSpread(cell),
      'the timestamp and its status badge must share a line, not stack',
    ).toBeLessThan(SAME_LINE);
  });

  /**
   * Sibling folders differ at their end, so a path cut short keeps its end:
   * `/mnt/storage/media/movies` and `/mnt/storage/media/movies-anime` must not
   * both read as the same start.
   */
  test('a path too long for its cell keeps its end and loses its start', async ({ page }) => {
    await page.goto('/media');

    const cell = page.locator('.cell-path').first();
    await expect(cell).toBeVisible();
    const cut = await cell.evaluate((node) => {
      const long = '/mnt/storage/media/library/movies-anime';
      const holder = node.querySelector('bdi') ?? node;
      holder.textContent = long;
      const run = holder.firstChild as Text;
      const box = node.getBoundingClientRect();
      const range = document.createRange();
      range.setStart(run, long.length - 1);
      range.setEnd(run, long.length);
      const last = range.getBoundingClientRect();
      range.setStart(run, 0);
      range.setEnd(run, 1);
      const first = range.getBoundingClientRect();
      return {
        overflows: node.scrollWidth > node.clientWidth,
        endShown: last.left >= box.left && last.right <= box.right + 1,
        startHidden: first.left < box.left,
      };
    });
    expect(cut).toEqual({ overflows: true, endShown: true, startHidden: true });
  });

  test('a timestamp is written the way the language writes it', async ({ page }) => {
    await page.goto('/instances');

    const cell = page.locator('td.cell-timestamp').first();
    // Not the stored form: that is a log line, not a date.
    await expect(cell).not.toHaveText(/\d{4}-\d{2}-\d{2} \d{2}:\d{2}:\d{2}/);
    // The exact value stays reachable for an audit, on the title attribute.
    await expect(cell).toHaveAttribute('title', /\d{4}-\d{2}-\d{2}/);
  });

  test('switching to a longer language does not fold the column', async ({ page }) => {
    // Greek and German run long: if any language folds the cell it is one of them.
    await api('/settings', {
      method: 'PUT',
      body: JSON.stringify({ settings: { ui_language: 'de' } }),
    });
    await page.goto('/instances');

    const cell = page.locator('td.cell-timestamp').first();
    await expect(cell).toBeVisible();
    expect(await verticalSpread(cell)).toBeLessThan(SAME_LINE);
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
      await page.setViewportSize({ width: 375, height: 812 });
      await openScreen(page, path);

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
    await page.goto('/settings#metadata');
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

  test('the navigation is a drawer that opens and closes', async ({ page }) => {
    await page.setViewportSize({ width: 375, height: 812 });
    await page.goto('/rules');

    const sidebar = page.locator('.sidebar');
    await expect(sidebar).not.toHaveClass(/is-open/);

    await page.getByRole('button', { name: 'Open navigation' }).click();
    await expect(sidebar).toHaveClass(/is-open/);
    await expect(page.getByRole('link', { name: /rules/i }).first()).toBeVisible();

    // Following a link closes it, so the page is not left under a scrim. By
    // href rather than by name: the label is translated and renameable, the
    // destination is not.
    await page.locator('.sidebar a[href="/media"]').click();
    await expect(sidebar).not.toHaveClass(/is-open/);
    await expect(page).toHaveURL(/\/media/);
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

    for (const path of ROUTES) {
      await openScreen(page, path);

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
    await openScreen(page, '/media');

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
});

test.describe('right-to-left', () => {
  /**
   * Arabic ships, so the mirroring is exercised for real in `rtl.spec.ts`, and
   * `languages.spec.ts` checks every screen in Arabic for sideways scroll.
   * This one flips `dir` on the English page instead: the risk lives in the
   * CSS, and this exercises exactly that. The shell is built from logical
   * properties (`inset-inline-start`, `margin-inline-start`, `text-align:
   * start`) so it mirrors on its own.
   */
  test('the shell mirrors instead of overlapping', async ({ page }) => {
    await page.setViewportSize({ width: 1280, height: 800 });
    await openScreen(page, '/media');

    const sideOf = () =>
      page.evaluate(() => {
        const sidebar = document.querySelector('.sidebar')!.getBoundingClientRect();
        const content = document.querySelector('.main-content')!.getBoundingClientRect();
        return { sidebarLeft: Math.round(sidebar.left), contentLeft: Math.round(content.left) };
      });

    const ltr = await sideOf();
    expect(ltr.sidebarLeft, 'the sidebar should start on the left').toBe(0);

    await page.evaluate(() => document.documentElement.setAttribute('dir', 'rtl'));
    const rtl = await sideOf();

    // The sidebar crosses to the other edge, and the content stops being
    // pushed away from the side the sidebar left.
    expect(rtl.sidebarLeft, 'the sidebar did not move to the right').toBeGreaterThan(500);
    expect(rtl.contentLeft, 'the content is still offset for a left sidebar').toBe(0);
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
  await expect(page.locator('tbody tr').first()).toBeVisible();

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
  await openScreen(page, '/logs');

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
    return { left: strip(1), right: strip(bitmap.width - 17), width: bitmap.width };
  }, Array.from(shot));

  // Each 16px strip must be one flat colour: a gradient means a shadow.
  for (const [side, pixels] of [
    ['left', edges.left],
    ['right', edges.right],
  ] as const) {
    const first = pixels.slice(0, 4).join();
    const varied = [];
    for (let i = 0; i < pixels.length; i += 4) {
      const px = pixels.slice(i, i + 4).join();
      if (px !== first) varied.push(`x=${i / 4} ${px}`);
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
    await page.goto('/health');
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
    await page.goto('/settings#routing');
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
    const tab = page.getByRole('tab', { name: 'Routing: a value is outside its bounds' });
    await expect(tab.locator('svg')).toBeVisible();
  });
});
