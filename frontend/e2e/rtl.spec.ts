import { test, expect, api } from './fixtures';

/**
 * The interface in a right-to-left language.
 *
 * The shell is written in logical properties, so it mirrors on its own. What
 * does not follow from the stylesheet is the second half: a table column mixes
 * our translated headings with values we did not write, and laid out in the
 * page's paragraph direction a path loses its leading slash to the right edge
 * and a date swaps its halves around the space between them. The characters
 * stay right, their order does not, and no amount of translation fixes it.
 *
 * Geometry is the only way to catch that: the text content is identical either
 * way, so a DOM assertion sees nothing. jsdom computes no layout, which is
 * why this lives here rather than in the component suite.
 */

async function setLanguage(code: string): Promise<void> {
  await api('/settings', {
    method: 'PUT',
    body: JSON.stringify({ settings: { ui_language: code } }),
  });
}

test.describe('right to left', () => {
  // Each test sets the language itself, never a `beforeAll`: the fixture's
  // reset before every test puts `ui_language` back to English, and a page
  // measured after it would be left to right.

  /**
   * A tick is not "towards" anything, so it must not mirror. Drawn with
   * logical borders, right to left keeps the other two and turns it into a
   * chevron.
   */
  test('a ticked checkbox draws the same tick right to left', async ({ page }) => {
    await setLanguage('ar');
    await page.goto('/settings#automation');

    const drawn = await page.evaluate(() => {
      const box = document.createElement('input');
      box.type = 'checkbox';
      box.checked = true;
      document.querySelector('main')!.append(box);
      const tick = getComputedStyle(box, '::after');
      const sides = {
        left: tick.borderLeftWidth,
        bottom: tick.borderBottomWidth,
        right: tick.borderRightWidth,
        top: tick.borderTopWidth,
      };
      box.remove();
      return { dir: document.documentElement.dir, ...sides };
    });
    expect(drawn).toEqual({ dir: 'rtl', left: '2px', bottom: '2px', right: '0px', top: '0px' });
  });

  /**
   * The select arrow and the facets chevron point down whatever the writing:
   * the arrow's two halves keep their order at the other end, and the chevron
   * keeps its physical sides.
   */
  test('a select arrow and the facets chevron still point down', async ({ page }) => {
    await setLanguage('ar');
    await page.goto('/rules');
    await expect(page.locator('.facet-panel > summary')).toBeVisible();

    const drawn = await page.evaluate(() => {
      const select = document.createElement('select');
      select.className = 'form-select';
      document.querySelector('main')!.append(select);
      const [first, second] = getComputedStyle(select)
        .backgroundPosition.split(',')
        .map((position) => parseFloat(position));
      select.remove();
      const chevron = getComputedStyle(
        document.querySelector('.facet-panel > summary')!,
        '::after',
      );
      return {
        halvesInOrder: first! < second!,
        right: chevron.borderRightWidth,
        bottom: chevron.borderBottomWidth,
      };
    });
    expect(drawn).toEqual({ halvesInOrder: true, right: '2px', bottom: '2px' });
  });

  /** A path is a machine format, read left to right whatever the page. */
  test('a path typed into a field runs left to right', async ({ page }) => {
    await setLanguage('ar');
    await page.goto('/root-folders');

    await expect(page.locator('#declare-path')).toHaveCSS('direction', 'ltr');
  });

  /** An arrow means "towards", so it turns around when the writing does. */
  test('every arrow in the explanation points the way the text reads', async ({ page }) => {
    await setLanguage('ar');
    await page.goto('/media');
    await page
      .getByRole('button', { name: /^لماذا؟ – / })
      .first()
      .click();

    const panel = page.getByRole('dialog');
    await expect(panel).toBeVisible();
    const arrows = await panel.evaluate((dialog) => {
      const walker = document.createTreeWalker(dialog, NodeFilter.SHOW_TEXT);
      const found: string[] = [];
      for (let node = walker.nextNode(); node; node = walker.nextNode()) {
        if (!node.textContent?.includes('→')) continue;
        const holder = node.parentElement?.closest('.dir-aware');
        found.push(holder ? getComputedStyle(holder).transform : 'not mirrored');
      }
      return found;
    });
    expect(arrows.length).toBeGreaterThan(0);
    expect(arrows.filter((transform) => transform !== 'matrix(-1, 0, 0, 1, 0, 0)')).toEqual([]);
  });

  /** Unmirrored, a proposed move points from the target back to the current folder. */
  test("a proposal's move arrow points the way the text reads", async ({ page }) => {
    await api('/rules', {
      method: 'POST',
      body: JSON.stringify({
        name: 'Akira',
        target_category: 'anime',
        media_type: 'movie',
        priority: 10,
        enabled: true,
        condition_logic: 'any',
        conditions: [{ type: 'title_contains', value: ['akira'] }],
        exclusions: [],
      }),
    });
    await setLanguage('ar');
    await page.goto('/simulation');
    await page.getByRole('button', { name: 'تشغيل المحاكاة' }).click();

    const arrow = page.locator('tbody svg.text-warning').first();
    await expect(arrow).toBeVisible();
    await expect(arrow).toHaveCSS('transform', 'matrix(-1, 0, 0, 1, 0, 0)');
  });

  test('the shell mirrors and nothing spills off the side', async ({ page }) => {
    await setLanguage('ar');

    for (const path of ['/', '/rules', '/root-folders', '/settings']) {
      await page.goto(path);
      await page.waitForLoadState('networkidle');

      const state = await page.evaluate(() => {
        const sidebar = document.querySelector('aside, nav, .sidebar');
        const content = document.querySelector('.main-content');
        return {
          direction: getComputedStyle(document.body).direction,
          arabic: /[؀-ۿ]/.test(document.body.innerText),
          sidebarLeft: sidebar ? sidebar.getBoundingClientRect().left : null,
          contentLeft: content ? Math.round(content.getBoundingClientRect().left) : null,
          viewport: window.innerWidth,
          overflow: document.documentElement.scrollWidth - document.documentElement.clientWidth,
        };
      });

      expect(state.direction, `${path} should be laid out right to left`).toBe('rtl');
      expect(state.arabic, `${path} should render Arabic text`).toBe(true);
      expect(state.overflow, `${path} should not scroll sideways`).toBeLessThanOrEqual(0);
      // Mirrored means the navigation is now on the trailing edge.
      expect(state.sidebarLeft, `${path} should put the sidebar on the right`).toBeGreaterThan(
        state.viewport / 2,
      );
      // And the content stops being pushed away from the side the sidebar left.
      expect(state.contentLeft, `${path} is still offset for a left sidebar`).toBe(0);
    }

    // `.mono` holds machine formats: paths, ids, condition summaries, raw
    // timestamps. Some carry no strongly-directional character at all, and for
    // those the `unicode-bidi: plaintext` rule on table cells is not enough:
    // with nothing to resolve, the paragraph direction wins and
    // `2026-08-26 14:02:30` comes out reversed.
    // Asserted as a style rather than as geometry because the case that needs
    // it is precisely the one with no letters to measure.
    await page.goto('/root-folders');
    await page.waitForLoadState('networkidle');
    const mono = page.locator('.mono').first();
    await expect(mono).toBeVisible();
    expect(await mono.evaluate((node) => getComputedStyle(node).direction)).toBe('ltr');
  });

  test('a Latin sentence keeps its full stop at its end', async ({ page, instanceId }) => {
    // On the rules table in Arabic, a description written in English must not
    // render as `.aimed at small children`. A full stop is direction-neutral,
    // so laid out in the page's paragraph direction it lands at the left end of
    // the line, where the English sentence begins. The characters are
    // identical either way, so only geometry sees it.
    const description = 'Anime, unless it is clearly aimed at small children.';
    await api('/rules', {
      method: 'POST',
      body: JSON.stringify({
        name: 'Japanese animation',
        description,
        target_category: 'anime',
        media_type: 'both',
        match_mode: 'all',
        priority: 10,
        enabled: true,
        instance_id: instanceId,
        conditions: [{ type: 'genre_contains', value: ['Animation'] }],
      }),
    });

    await setLanguage('ar');
    await page.goto('/rules');
    await page.waitForLoadState('networkidle');
    await expect
      .poll(() => page.evaluate(() => getComputedStyle(document.body).direction))
      .toBe('rtl');

    const cell = page.getByText(description, { exact: true });
    await expect(cell).toBeVisible();

    const run = await cell.evaluate((node, text: string) => {
      const target = node.firstChild;
      if (!target || target.nodeType !== Node.TEXT_NODE) return null;
      const box = (from: number, to: number) => {
        const range = document.createRange();
        range.setStart(target, from);
        range.setEnd(target, to);
        return range.getBoundingClientRect();
      };
      // The last two characters: `n` then the full stop that belongs after it.
      const letter = box(text.length - 2, text.length - 1);
      const stop = box(text.length - 1, text.length);
      return {
        letterLeft: letter.left,
        stopLeft: stop.left,
        sameLine: Math.abs(letter.top - stop.top) < 4,
      };
    }, description);

    expect(run, 'the description should be one text node').not.toBeNull();
    expect(run!.sameLine, 'the last two characters should share a line').toBe(true);
    expect(run!.stopLeft, 'the full stop moved to the leading edge').toBeGreaterThan(
      run!.letterLeft,
    );
  });
});
