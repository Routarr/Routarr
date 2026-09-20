import { test, expect, api } from './fixtures';

/**
 * The interface in a right-to-left language.
 *
 * The shell is written in logical properties, so it mirrors on its own — that
 * part only needed looking at once. What does not follow from the stylesheet is
 * the second half: a table column mixes our translated headings with values we
 * did not write, and laid out in the page's paragraph direction a path loses
 * its leading slash to the right edge and a date swaps its halves around the
 * space between them. The characters stay right, their order does not, and no
 * amount of translation fixes it.
 *
 * Geometry is the only way to catch that: the text content is identical either
 * way, so a DOM assertion sees nothing. happy-dom computes no layout, which is
 * why this lives here rather than in the component suite.
 *
 * The language is a global setting and the suite runs serial on one server, so
 * it is put back afterwards rather than left for the next spec to find.
 */

async function setLanguage(code: string): Promise<void> {
  await api('/settings', {
    method: 'PUT',
    body: JSON.stringify({ settings: { ui_language: code } }),
  });
}

test.describe('right to left', () => {
  // Not `beforeAll`: the `instanceId` fixture resets the library before every
  // test, and its reset puts `ui_language` back to English. Set ahead of it,
  // the language is undone before the first navigation and every assertion
  // below measures a left-to-right page while passing.
  test.afterAll(async () => {
    await setLanguage('en');
  });

  test('the shell mirrors and nothing spills off the side', async ({ page, instanceId }) => {
    expect(instanceId).toBeTruthy();
    await setLanguage('ar');

    for (const path of ['/', '/rules', '/root-folders', '/settings']) {
      await page.goto(path);
      await page.waitForLoadState('networkidle');

      const state = await page.evaluate(() => {
        const sidebar = document.querySelector('aside, nav, .sidebar');
        return {
          direction: getComputedStyle(document.body).direction,
          arabic: /[؀-ۿ]/.test(document.body.innerText),
          sidebarLeft: sidebar ? sidebar.getBoundingClientRect().left : null,
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
    }

    // `.mono` holds machine formats — paths, ids, condition summaries, raw
    // timestamps. Some carry no strongly-directional character at all, and for
    // those the cell rule below is not enough: with nothing to resolve, the
    // paragraph direction wins and `2026-08-26 14:02:30` comes out reversed.
    // Asserted as a style rather than as geometry because the case that needs
    // it is precisely the one with no letters to measure.
    await page.goto('/root-folders');
    await page.waitForLoadState('networkidle');
    const mono = page.locator('.mono').first();
    await expect(mono).toBeVisible();
    expect(await mono.evaluate((node) => getComputedStyle(node).direction)).toBe('ltr');
  });

  test('a Latin sentence keeps its full stop at its end', async ({ page, instanceId }) => {
    // The defect this guards, seen on the rules table in Arabic: a description
    // written in English rendered as `.aimed at small children`. A full stop is
    // direction-neutral, so laid out in the page's paragraph direction it is
    // pulled to the leading edge — which in a right-to-left page is the left.
    // The characters are identical either way, so only geometry sees it.
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
