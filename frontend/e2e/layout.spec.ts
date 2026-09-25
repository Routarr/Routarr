import type { Locator } from '@playwright/test';

import { test, expect, api } from './fixtures';

/**
 * Layout facts that only a real browser can establish. happy-dom computes no
 * geometry, so the component suite can assert that markup exists but never that
 * it fits — which is exactly where the defects were.
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
  test('the last-sync column does not stack its date and status', async ({ page, instanceId }) => {
    expect(instanceId).toBeTruthy();
    await page.goto('/instances');

    const cell = page.locator('td.cell-timestamp').first();
    await expect(cell).toBeVisible();
    await expect(cell.locator('.badge')).toBeVisible();

    expect(
      await verticalSpread(cell),
      'the timestamp and its status badge must share a line, not stack',
    ).toBeLessThan(SAME_LINE);
  });

  test('a timestamp is written the way the language writes it', async ({ page, instanceId }) => {
    expect(instanceId).toBeTruthy();
    await page.goto('/instances');

    const cell = page.locator('td.cell-timestamp').first();
    // Not the stored form: that is a log line, not a date.
    await expect(cell).not.toHaveText(/\d{4}-\d{2}-\d{2} \d{2}:\d{2}:\d{2}/);
    // The exact value stays reachable for an audit, on the title attribute.
    await expect(cell).toHaveAttribute('title', /\d{4}-\d{2}-\d{2}/);
  });

  test('switching to a longer language does not fold the column', async ({ page, instanceId }) => {
    expect(instanceId).toBeTruthy();
    // Greek and German run long; if any language folds the cell it is one of them.
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

test.describe('the page never scrolls sideways', () => {
  for (const path of ['/', '/instances', '/rules', '/media', '/simulation', '/history', '/jobs']) {
    test(`${path} fits its viewport`, async ({ page, instanceId }) => {
      expect(instanceId).toBeTruthy();
      await page.setViewportSize({ width: 1280, height: 800 });
      await page.goto(path);
      await expect(page.locator('.page-title')).toBeVisible();

      // A wide table is fine — it scrolls inside .table-container. The body
      // scrolling is what looks broken.
      const overflow = await page.evaluate(
        () => document.documentElement.scrollWidth - document.documentElement.clientWidth,
      );
      expect(overflow, `${path} scrolls horizontally by ${overflow}px`).toBeLessThanOrEqual(1);
    });
  }
});

/**
 * A phone. Below the drawer breakpoint the navigation leaves the flow, and a
 * table has to scroll inside its own region rather than drag the page with it.
 * Nothing in the unit tests computes layout; this is where a 375px screen is
 * seen at all.
 */
test.describe('on a phone', () => {
  const SCREENS = [
    '/',
    '/instances',
    '/root-folders',
    '/rules',
    '/rules/tests',
    '/media',
    '/simulation',
    '/history',
    '/overrides',
    '/jobs',
    '/logs',
    '/health',
    '/settings',
  ];

  for (const path of SCREENS) {
    test(`${path} fits a 375px screen and its tables scroll inside their region`, async ({
      page,
      instanceId,
    }) => {
      expect(instanceId).toBeTruthy();
      await page.setViewportSize({ width: 375, height: 812 });
      await page.goto(path);
      await expect(page.locator('h1').first()).toBeVisible();

      const overflow = await page.evaluate(
        () => document.documentElement.scrollWidth - document.documentElement.clientWidth,
      );
      expect(overflow, `${path} scrolls the page sideways by ${overflow}px`).toBeLessThanOrEqual(1);

      // A table wider than the screen is expected; the region around it is what
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
   * `/settings` opens on its first tab, so no test had ever rendered the
   * source list narrow. Holding its three lanes, the actions kept the 190px
   * that aligns their right edges, the credential field beside them measured
   * 51px — narrower than the word it holds — and "Disable" was clipped 50px
   * outside its own row. The document never scrolled, so the check above
   * passed throughout; nothing but a measurement inside the row shows it.
   */
  test('the source list stacks rather than squeezing its own field', async ({
    page,
    instanceId,
  }) => {
    expect(instanceId).toBeTruthy();
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
   * The test above has always been here and has always passed, because the
   * fixture never reaches the worst case: the bar's width was a function of
   * how many counts were non-zero *and* of how long the language writes them.
   * Forced, it measured 168px of horizontal document overflow at 360px, with
   * badges 80px tall inside a 64px bar. A fixture that cannot produce the
   * failure is a test that cannot find it.
   */
  test('every screen fits 360px with every counter at once', async ({ page, instanceId }) => {
    expect(instanceId).toBeTruthy();
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
          warnings: ['a', 'b', 'c', 'd'],
        },
      });
    });
    await page.setViewportSize({ width: 360, height: 780 });

    for (const path of SCREENS) {
      await page.goto(path);
      await expect(page.locator('h1').first()).toBeVisible();

      const measured = await page.evaluate(() => {
        const bar = document.querySelector('.topbar');
        return {
          overflow: document.documentElement.scrollWidth - document.documentElement.clientWidth,
          // Nothing in the bar may be taller than the bar: a wrapped label is
          // how the chrome burst in the first place.
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

  test('the navigation is a drawer that opens and closes', async ({ page, instanceId }) => {
    expect(instanceId).toBeTruthy();
    await page.setViewportSize({ width: 375, height: 812 });
    await page.goto('/rules');

    const sidebar = page.locator('.sidebar');
    await expect(sidebar).not.toHaveClass(/is-open/);

    await page.locator('.sidebar-toggle').click();
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
  const PAGES = [
    '/',
    '/instances',
    '/root-folders',
    '/rules',
    '/media',
    '/simulation',
    '/history',
    '/overrides',
    '/jobs',
    '/logs',
    '/health',
    '/settings',
  ];

  /**
   * Without a fixed height the same `btn btn-primary` renders at three sizes:
   * a flex row stretches buttons to their tallest sibling, `<label class="btn">`
   * inherits a line-height `<button>` resets, and a text label makes a taller
   * line box than an icon. All three are invisible to a unit test.
   */
  test('every button on every page shares its size', async ({ page, instanceId }) => {
    expect(instanceId).toBeTruthy();

    const regular = new Map<number, string>();
    const small = new Map<number, string>();

    for (const path of PAGES) {
      await page.goto(path);
      await expect(page.locator('.page-title')).toBeVisible();

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
    expect([...regular.keys()][0]).toBeGreaterThan([...small.keys()][0]);
  });
});

test.describe('the chrome draws one line', () => {
  /**
   * The sidebar header and the top bar sit side by side and each ends in a
   * rule, so together they read as one line across the window — unless their
   * heights differ, which puts a step in the middle of it. They did: 72px
   * against 64px, because one was sized by its logo and the other was fixed.
   *
   * Only a browser can measure where a border actually lands.
   */
  test('the sidebar header and the top bar end at the same height', async ({ page }) => {
    await page.goto('/media');
    await expect(page.locator('.page-title')).toBeVisible();

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
  const PAGES = ['/instances', '/media', '/history'];

  /**
   * Arabic ships, so the mirroring is exercised for real in `rtl.spec.ts`,
   * which drives the language setting end to end. These stay because they
   * measure something that one does not: they flip `dir` on the English page,
   * so a `margin-left` added later breaks them on the screens the Arabic
   * journey does not visit. The shell is built from
   * logical properties (`inset-inline-start`, `margin-inline-start`,
   * `text-align: start`) so it mirrors on its own.
   *
   * The direction is forced here rather than chosen through a language: the
   * risk lives in the CSS, and this exercises exactly that.
   */
  test('the shell mirrors instead of overlapping', async ({ page, instanceId }) => {
    expect(instanceId).toBeTruthy();
    await page.setViewportSize({ width: 1280, height: 800 });
    await page.goto('/media');
    await expect(page.locator('.page-title')).toBeVisible();

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

    // The sidebar crosses to the other edge, and the content is no longer
    // pushed away from the side the sidebar left.
    expect(rtl.sidebarLeft, 'the sidebar did not move to the right').toBeGreaterThan(500);
    expect(rtl.contentLeft, 'the content is still offset for a left sidebar').toBe(0);
  });

  for (const path of PAGES) {
    test(`${path} does not scroll sideways in rtl`, async ({ page, instanceId }) => {
      expect(instanceId).toBeTruthy();
      await page.setViewportSize({ width: 1280, height: 800 });
      await page.goto(path);
      await expect(page.locator('.page-title')).toBeVisible();
      await page.evaluate(() => document.documentElement.setAttribute('dir', 'rtl'));

      // A hard-coded left offset shows up here first: the shell ends up one
      // sidebar wider than the window.
      const overflow = await page.evaluate(
        () => document.documentElement.scrollWidth - document.documentElement.clientWidth,
      );
      expect(overflow, `${path} overflows by ${overflow}px in rtl`).toBeLessThanOrEqual(1);
    });
  }
});

/**
 * A table that does not scroll shows no scroll shadow.
 *
 * The shadows are painted as four background layers on the scroll container:
 * two "covers" pinned to the content (`background-attachment: local`) and two
 * shadows pinned to the frame (`scroll`), so each shadow is hidden until there
 * is something in that direction. It only works if the cover is *opaque* across
 * the width of the shadow it hides — and it was a gradient fading from the very
 * first pixel, so the shadow showed through at roughly half strength for its
 * whole 14px. Every table with nothing to scroll, which is most of them at a
 * desktop width, carried a permanent dark band down both edges.
 *
 * Measured rather than eyeballed: nothing about this is visible to a DOM query,
 * and it survived a full UI review because it reads as a design choice until
 * you sample the pixels.
 */
/**
 * Ticking a box may not move the page under the pointer.
 *
 * The checkbox is an `inline-grid`, and its tick only exists when it is
 * checked: with no in-flow child the box synthesises its baseline from the
 * bottom of its margin box, so the two states made line boxes of different
 * heights. The table head measured 39px unchecked and 35px checked, and
 * selecting all lifted every row below it by 4px — under the very pointer that
 * had just clicked. Only a head shows it, a body row being as tall as its text,
 * and no DOM query can see it at all.
 */
test('selecting rows does not move the rows', async ({ page, instanceId }) => {
  expect(instanceId).toBeTruthy();
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

  const selectAll = page.locator('thead input[type="checkbox"]').first();
  const firstRow = page.locator('tbody tr').first();

  // The cell holding the box and nothing else. Measuring the whole head hides
  // the defect wherever a longer heading is the taller thing in that row —
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

test('a table with nothing to scroll has no shadow down its edges', async ({
  page,
  instanceId,
}) => {
  expect(instanceId).toBeTruthy();

  await page.goto('/logs');
  await page.locator('h1').first().waitFor({ state: 'visible' });

  const container = page.locator('.table-container').first();
  await container.waitFor({ state: 'visible' });

  const overflows = await container.evaluate((el) => el.scrollWidth > el.clientWidth);
  expect(overflows, 'the viewport is too narrow for this test to mean anything').toBe(false);

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
