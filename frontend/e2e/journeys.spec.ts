import { test, expect, api } from './fixtures';

/**
 * The journeys a user actually walks, against the real binary. These exist to
 * catch what neither the Rust tests nor the component tests can see: a route
 * that does not resolve, a control wired to nothing, a table that renders empty
 * because a payload field was renamed on one side only.
 */

test.describe('navigation', () => {
  test('every sidebar entry reaches a page that renders', async ({ page, instanceId }) => {
    expect(instanceId).toBeTruthy();
    await page.goto('/');

    // Read the entries from the sidebar itself rather than restating the route
    // table here: a link added without a page must fail this test.
    const links = page.locator('.sidebar-nav a');
    const count = await links.count();
    expect(count).toBeGreaterThan(5);

    for (let index = 0; index < count; index += 1) {
      const link = links.nth(index);
      const label = (await link.innerText()).trim();
      await link.click();

      // A rendered page has a non-empty title, and no error banner.
      await expect(page.getByRole('heading', { level: 1 }), `${label} has no title`).toBeVisible();
      await expect(
        page.getByRole('heading', { level: 1 }),
        `${label} has an empty title`,
      ).not.toHaveText('');
      // `banner-danger` is the class ErrorBanner actually renders; asserting on
      // one that does not exist would pass on every page, broken or not.
      await expect(page.getByRole('alert'), `${label} shows an error`).toHaveCount(0);
    }
  });

  test('the chrome speaks the configured language', async ({ page, instanceId }) => {
    expect(instanceId).toBeTruthy();
    // Proves the dictionary reaches the browser, which the English run cannot:
    // in English most keys and their translations are the same word.
    await api('/settings', {
      method: 'PUT',
      body: JSON.stringify({ settings: { ui_language: 'fr' } }),
    });
    await page.goto('/');

    await expect(page.locator('.sidebar-nav')).toContainText('Réglages');
    await expect(page.locator('.topbar')).toContainText(/simulation|réel/i);
  });

  test('a deep link survives a reload', async ({ page, instanceId }) => {
    expect(instanceId).toBeTruthy();
    // Without the index fallback in the static handler this is a 404.
    await page.goto('/rules');
    await page.reload();

    await expect(page.getByRole('heading', { level: 1 })).toBeVisible();
  });

  /**
   * The same index fallback that makes a deep link work also means the server
   * answers 200 for *any* path, so telling somebody they mistyped is the
   * client's job. It used to render the dashboard instead — a screen nobody
   * asked for, with no menu entry active and nothing saying why.
   */
  test('a path that does not exist says so, and keeps the address', async ({ page }) => {
    await page.goto('/typo');

    await expect(page.getByRole('heading', { level: 1 })).toHaveText(/introuvable|not found/i);
    // Kept, so it can be corrected or reported. A redirect would erase it.
    expect(new URL(page.url()).pathname).toMatch(/\/typo$/);
    // Inside the shell: the menu is the way out.
    await expect(page.locator('.sidebar-nav')).toBeVisible();
    await expect(page.locator('.sidebar-nav .active')).toHaveCount(0);

    // Scoped: the sidebar has a link to the same place, and the way out being
    // offered here is the one on the page.
    await page.locator('.empty-state').getByRole('link').click();
    await expect(page.getByRole('heading', { level: 1 })).toBeVisible();
    expect(new URL(page.url()).pathname).toMatch(/\/$/);
  });
});

test.describe('picking a condition value', () => {
  /**
   * A genre is a string the engine compares, and the exact spelling lives in
   * the metadata rather than in the user's head. Only a browser can establish
   * that the library's own values reach the picker, that choosing one stores
   * that value, and that a second one is an alternative rather than a second
   * requirement.
   */
  test('a genre rule is built from the library, not from memory', async ({ page, instanceId }) => {
    expect(instanceId).toBeTruthy();
    await page.goto('/rules');
    await page.getByRole('button', { name: 'New Rule' }).click();

    const group = page.locator('.form-group').filter({ hasText: 'Rule name' }).first();
    await group.locator('input').fill('Animated');
    await page
      .locator('.form-group')
      .filter({ hasText: 'Target category' })
      .first()
      .locator('select')
      .selectOption('anime');

    const conditions = page
      .locator('.form-group')
      .filter({ hasText: 'Conditions (all of)' })
      .first();
    await conditions.locator('select').selectOption('genre_contains');

    // The list is what the fake Radarr's library carries, so a value appears
    // here only if the facets reached the browser. Named exactly: the
    // quantifier selector beside it is captioned with the same words.
    const picker = page.getByRole('combobox', { name: 'Genre contains', exact: true });
    await picker.click();
    await expect(page.getByRole('option', { name: 'Animation' })).toBeVisible();

    // Typed without its capital, and the option is still found: the point of
    // the control is that the spelling is not the user's to remember.
    await picker.fill('anim');
    await page.getByRole('option', { name: 'Animation' }).click();

    await page.getByRole('button', { name: 'Save rule' }).click();
    await expect(page.getByText('Animated')).toBeVisible();

    // What was stored is the library's spelling, in the multi-value shape.
    const rules = (await api('/rules')) as { name: string; conditions: unknown[] }[];
    const saved = rules.find((rule) => rule.name === 'Animated');
    expect(saved?.conditions).toEqual([{ type: 'genre_contains', value: ['Animation'] }]);
  });

  /**
   * OR and AND on one condition. It is the distinction the form could not make
   * before without asking for a second condition, and only a browser shows that
   * turning the selector keeps the values that were already chosen.
   */
  test('one condition can ask for either genre or for both', async ({ page, instanceId }) => {
    expect(instanceId).toBeTruthy();
    await page.goto('/rules');
    await page.getByRole('button', { name: 'New Rule' }).click();

    await page
      .locator('.form-group')
      .filter({ hasText: 'Rule name' })
      .first()
      .locator('input')
      .fill('Both genres');
    await page
      .locator('.form-group')
      .filter({ hasText: 'Target category' })
      .first()
      .locator('select')
      .selectOption('anime');
    await page
      .locator('.form-group')
      .filter({ hasText: 'Conditions (all of)' })
      .first()
      .locator('select')
      .first()
      .selectOption('genre_contains');

    const picker = page.getByRole('combobox', { name: 'Genre contains', exact: true });
    // Both are carried by the fixture's library, so both come from the facets.
    for (const genre of ['Animation', 'Family']) {
      await picker.fill(genre);
      await page.getByRole('option', { name: genre, exact: true }).click();
    }

    // Both values are on one condition, so this is the OR.
    const quantifier = page.getByRole('combobox', { name: /how these values combine/i });
    await expect(quantifier).toHaveValue('any');

    // One turn of the selector makes it the AND, and the chosen genres stay.
    await quantifier.selectOption('all');
    // Scoped to the chips: the facets panel behind the dialog names the same
    // genres, and a page-wide query matches those too.
    await expect(page.locator('dialog[open] .chip')).toHaveText(['Animation', 'Family']);

    await page.getByRole('button', { name: 'Save rule' }).click();
    await expect(page.getByText('Both genres')).toBeVisible();

    const rules = (await api('/rules')) as { name: string; conditions: unknown[] }[];
    expect(rules.find((rule) => rule.name === 'Both genres')?.conditions).toEqual([
      { type: 'genre_contains_all', value: ['Animation', 'Family'] },
    ]);
  });
});

test.describe('the routing journey', () => {
  test('create a rule, simulate, apply, then revert', async ({ page, instanceId }) => {
    expect(instanceId).toBeTruthy();
    await api('/settings', {
      method: 'PUT',
      body: JSON.stringify({ settings: { global_dry_run: 'false' } }),
    });

    // --- create the rule through the editor -------------------------------
    // Controls are addressed through their labels rather than by position, so
    // reordering the form does not silently retarget the test.
    const field = (label: string) => page.locator('.form-group').filter({ hasText: label }).first();

    await page.goto('/rules');
    await page.getByRole('button', { name: 'New Rule' }).click();

    await field('Rule name').locator('input').fill('Japanese animation');
    await field('Target category').locator('select').selectOption('anime');

    // Conditions are added by picking from a select, not by clicking a button.
    await field('Conditions (all of)').locator('select').selectOption('title_contains');
    await page.locator('.condition-row input.form-input').fill('akira, totoro, perfect blue');

    await page.getByRole('button', { name: 'Save rule' }).click();
    await expect(page.getByText('Japanese animation')).toBeVisible();

    // --- simulate ----------------------------------------------------------
    await page.goto('/simulation');
    await page.getByRole('button', { name: /run simulation/i }).click();

    // Matched on the title cell, not on the row: every row's explanation echoes
    // the condition's values, so a row-level text filter matches all of them.
    await expect(page.locator('tbody strong', { hasText: /^Akira$/ })).toHaveCount(1);

    // The explanation is the product's core promise, so it has to be on screen
    // next to the move rather than a click away.
    await expect(page.getByText(/Title contains .*found \[Akira\]/)).toBeVisible();
    await expect(page.getByText('/movies/anime').first()).toBeVisible();

    // --- apply -------------------------------------------------------------
    // Nothing is asked here, and that is the design: "apply selected" goes
    // straight through below the confirmation threshold, and only the backend's
    // refusal past it turns into a question. Asserting that no dialog appears
    // needs the absence to be observable, which a `page.on('dialog')` handler
    // cannot give: it cannot tell a dialog that never came from one it
    // accepted.
    await page.locator('tbody input[type="checkbox"]').first().check();
    await page.getByRole('button', { name: /apply selected/i }).click();

    await expect(page.locator('.banner-success')).toBeVisible();

    // It really reached the Arr, not just Routarr's own tables.
    const movies = (await (
      await fetch(`${process.env.ROUTARR_E2E_ARR ?? 'http://127.0.0.1:7979'}/api/v3/movie`)
    ).json()) as { title: string; rootFolderPath: string }[];
    const akira = movies.find((m) => m.title === 'Akira');
    expect(akira?.rootFolderPath).toBe('/movies/anime');

    // --- revert ------------------------------------------------------------
    await page.goto('/history');
    await page
      .getByRole('button', { name: /revert/i })
      .first()
      .click();

    // Reverting asks once, in a dialog that also carries the "move the files
    // too" choice, rather than two chained native prompts whose second Cancel
    // would not cancel.
    const confirmRevert = page.getByRole('dialog');
    await expect(confirmRevert).toBeVisible();
    await confirmRevert.getByRole('button', { name: /revert/i }).click();

    await expect(page.locator('.banner-success')).toBeVisible();

    // Back where it started, upstream as well as locally.
    const reverted = (await (
      await fetch(`${process.env.ROUTARR_E2E_ARR ?? 'http://127.0.0.1:7979'}/api/v3/movie`)
    ).json()) as { title: string; rootFolderPath: string }[];
    expect(reverted.find((m) => m.title === 'Akira')?.rootFolderPath).toBe('/movies/standard');
  });
});

test.describe('guardrails are visible', () => {
  test('the shell states whether writing is possible', async ({ page, instanceId }) => {
    expect(instanceId).toBeTruthy();
    await page.goto('/');
    // Scoped to the top bar: the Dashboard also has a "Run a dry-run" link, and
    // matching that instead would make this assertion pass for the wrong reason.
    //
    // Asserted on the accessible name rather than the visible text: the chip
    // shows a short word so the chrome does not change width with the
    // language, and the whole sentence is what a screen reader is given.
    const mode = page.locator('.topbar .mode-chip');
    await expect(mode).toHaveAccessibleName(/dry-run/i);
    await expect(page.locator('.topbar .mode-dot')).toHaveClass(/is-held/);

    await api('/settings', {
      method: 'PUT',
      body: JSON.stringify({ settings: { global_dry_run: 'false' } }),
    });
    await page.reload();
    await expect(mode).toHaveAccessibleName(/live mode/i);
    // Never the danger colour for the state the application is meant to run
    // in. Measured rather than asserted against a class name: `.mode-chip`
    // carries no modifier, so a class assertion here could not fail whatever
    // the stylesheet did. Both values are resolved by the browser, or a hex
    // token would be compared against an `rgb()` and never match either.
    const dot = page.locator('.topbar .mode-dot');
    await expect(dot).toHaveClass(/is-live/);
    const [painted, success, danger] = await page.evaluate(() => {
      const resolve = (token: string) => {
        const probe = document.createElement('span');
        probe.style.color = getComputedStyle(document.documentElement).getPropertyValue(token);
        document.body.append(probe);
        const value = getComputedStyle(probe).color;
        probe.remove();
        return value;
      };
      const mark = document.querySelector('.topbar .mode-dot');
      return [
        mark ? getComputedStyle(mark).backgroundColor : '',
        resolve('--status-success'),
        resolve('--status-danger'),
      ];
    });
    expect(painted).toBe(success);
    expect(painted).not.toBe(danger);
  });

  test('arming automatic application warns before it can write', async ({ page, instanceId }) => {
    expect(instanceId).toBeTruthy();
    // The two switches live in different sections, which is itself worth
    // exercising: the draft has to survive moving between them, or arming both
    // would be impossible in one visit.
    await page.goto('/settings#automation');
    await page.getByLabel('Automatic application').selectOption('true');

    // Automatic application alone is inert while dry-run holds, so it alone
    // must not warn.
    await expect(page.locator('.banner-warning')).toHaveCount(0);

    await page.getByRole('tab', { name: /routing/i }).click();
    await page.getByLabel('Global dry-run').selectOption('false');

    // Live mode, plus automatic application: the combination writes unattended,
    // and only the combination warns.
    await expect(page.locator('.banner-warning')).toHaveCount(2);
  });
});

test.describe('reclassifying a whole library', () => {
  test('apply all reaches past the displayed list, in batches', async ({ page, instanceId }) => {
    expect(instanceId).toBeTruthy();
    await api('/settings', {
      method: 'PUT',
      // Two per batch, so three matching films need two batches: the point is
      // that the user confirms once, not once per batch.
      body: JSON.stringify({ settings: { global_dry_run: 'false', batch_limit: '2' } }),
    });
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
    await expect(page.locator('tbody strong', { hasText: /^Akira$/ })).toHaveCount(1);

    await page.getByRole('button', { name: /apply all/i }).click();
    await page.locator('dialog[open]').getByRole('button', { name: 'Apply' }).click();

    // One banner, naming how many batches it took — not one dialog per batch.
    const banner = page.locator('.banner-success');
    await expect(banner).toBeVisible();
    await expect(banner).toContainText('3');

    // All three really moved, including the ones a single batch could not hold.
    const movies = (await (
      await fetch(`${process.env.ROUTARR_E2E_ARR ?? 'http://127.0.0.1:7979'}/api/v3/movie`)
    ).json()) as { title: string; rootFolderPath: string }[];
    const moved = movies.filter((m) => m.rootFolderPath === '/movies/anime').map((m) => m.title);
    expect(moved.sort()).toEqual(['Akira', 'My Neighbor Totoro', 'Perfect Blue']);
  });
});
