import type { Page } from '@playwright/test';

import {
  test,
  expect,
  api,
  apiWhenFree,
  openScreen,
  screenShown,
  writesDuring,
  ARR,
} from './fixtures';

/**
 * The journeys a user actually walks, against the real binary. These exist to
 * catch what neither the Rust tests nor the component tests can see: a route
 * that does not resolve, a control wired to nothing, a table that renders empty
 * because a payload field was renamed on one side only.
 */

test.describe('navigation', () => {
  test('every sidebar entry reaches a page that renders', async ({ page }) => {
    // Counted by hand: a sidebar link navigates without loading a page, so
    // `waitForLoadState('networkidle')` answers at once, before the loads.
    let inFlight = 0;
    page.on('request', () => (inFlight += 1));
    page.on('requestfinished', () => (inFlight -= 1));
    page.on('requestfailed', () => (inFlight -= 1));
    await page.goto('/');

    // Read the entries from the sidebar itself rather than restating the route
    // table here: a link added without a page must fail this test.
    const links = page.getByRole('navigation', { name: 'Main navigation' }).getByRole('link');
    const count = await links.count();
    expect(count).toBeGreaterThan(5);

    for (let index = 0; index < count; index += 1) {
      const link = links.nth(index);
      const label = (await link.innerText()).trim();
      await link.click();

      // A rendered page has its own title, not the not-found page's, and no
      // error banner once its loads have answered: the heading draws before
      // them, and a load answering 500 shows its banner after.
      await screenShown(page, label);
      await expect.poll(() => inFlight, { message: `${label} never settles` }).toBe(0);
      await expect(page.getByRole('alert'), `${label} shows an error`).toHaveCount(0);
    }
  });

  test('the chrome speaks the configured language', async ({ page }) => {
    // Proves the dictionary reaches the browser, which the English run cannot:
    // in English most keys and their translations are the same word.
    await api('/settings', {
      method: 'PUT',
      body: JSON.stringify({ settings: { ui_language: 'fr' } }),
    });
    await page.goto('/');

    await expect(
      page.getByRole('navigation', { name: 'Navigation principale' }).getByRole('link', {
        name: 'Réglages',
      }),
    ).toBeVisible();
    await expect(
      page.getByRole('status', { name: 'Essai à blanc : écritures bloquées' }),
    ).toBeVisible();
  });

  /**
   * The same index fallback that makes a deep link work also means the server
   * answers 200 for *any* path, so telling somebody they mistyped is the
   * client's job. The dashboard in its place would be a screen nobody asked
   * for, with no menu entry active and nothing saying why.
   */
  test('a path that does not exist says so, and keeps the address', async ({ page }) => {
    await page.goto('/typo');

    await expect(page.getByRole('heading', { level: 1 })).toHaveText('Page not found');
    // Kept, so it can be corrected or reported. A redirect would erase it.
    expect(new URL(page.url()).pathname).toMatch(/\/typo$/);
    // Inside the shell: the menu is the way out, with no entry marked current.
    const navigation = page.getByRole('navigation', { name: 'Main navigation' });
    await expect(navigation).toBeVisible();
    await expect(navigation.locator('[aria-current="page"]')).toHaveCount(0);

    // The way out offered in the page's header, where every action is, and
    // not the sidebar's own link to the same place.
    await page.getByRole('link', { name: 'Back to the dashboard' }).click();
    await screenShown(page, 'the dashboard');
    expect(new URL(page.url()).pathname).toMatch(/\/$/);
  });
});

/** Add a condition of `kind` to the rule the editor holds, through its own controls. */
async function addCondition(page: Page, kind: string): Promise<void> {
  const list = 'Conditions (all of)';
  await page.getByRole('combobox', { name: list, exact: true }).selectOption(kind);
  await page.getByRole('button', { name: `Add – ${list}` }).click();
}

test.describe('unsaved work', () => {
  /**
   * The browser has moved by the time Back is heard. Only a browser shows that
   * the editor asks first, and that Cancel puts the address back with the
   * draft still open.
   */
  test('Back under a half-written rule asks first, and Cancel keeps it', async ({ page }) => {
    await openScreen(page, '/');
    await page.getByRole('navigation').getByRole('link', { name: 'Rules', exact: true }).click();
    await page.getByRole('button', { name: 'New Rule' }).click();
    const name = page.getByRole('dialog', { name: 'Create routing rule' }).getByLabel('Rule name', {
      exact: true,
    });
    await name.fill('Draft');

    await page.goBack();
    const question = page.getByRole('alertdialog', { name: 'Discard' });
    await expect(question).toHaveAccessibleDescription(
      'Close the rule without saving your changes?',
    );
    await question.getByRole('button', { name: 'Cancel' }).click();

    await expect(question).toHaveCount(0);
    await expect(page).toHaveURL(/\/rules$/);
    await expect(name).toHaveValue('Draft');
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
  test('a genre rule is built from the library, not from memory', async ({ page }) => {
    await page.goto('/rules');
    await page.getByRole('button', { name: 'New Rule' }).click();

    await page.getByLabel('Rule name', { exact: true }).fill('Animated');
    await page.getByLabel('Target category', { exact: true }).selectOption('anime');
    await addCondition(page, 'genre_contains');

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
   * The editor is a modal dialog, where an Escape nobody claims closes it and
   * the draft with it. Folding the suggestions is what Escape does in a picker.
   */
  test('Escape in a picker folds its list and keeps the draft', async ({ page }) => {
    await page.goto('/rules');
    await page.getByRole('button', { name: 'New Rule' }).click();
    const dialog = page.getByRole('dialog');
    const name = dialog.getByLabel('Rule name', { exact: true });
    await name.fill('Draft');
    await addCondition(page, 'genre_contains');

    const picker = page.getByRole('combobox', { name: 'Genre contains', exact: true });
    await picker.fill('anim');
    await expect(page.getByRole('option', { name: 'Animation' })).toBeVisible();
    await picker.press('Escape');

    await expect(page.getByRole('option', { name: 'Animation' })).toHaveCount(0);
    await expect(dialog).toBeVisible();
    await expect(name).toHaveValue('Draft');
  });

  /**
   * OR and AND on one condition, with no second condition needed for the AND.
   * Only a browser shows that turning the selector keeps the values that were
   * already chosen.
   */
  test('one condition can ask for either genre or for both', async ({ page }) => {
    await page.goto('/rules');
    await page.getByRole('button', { name: 'New Rule' }).click();

    await page.getByLabel('Rule name', { exact: true }).fill('Both genres');
    await page.getByLabel('Target category', { exact: true }).selectOption('anime');
    await addCondition(page, 'genre_contains');

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
    await expect(
      page.getByRole('list', { name: 'Selected values' }).getByRole('listitem'),
    ).toHaveText(['Animation', 'Family']);

    await page.getByRole('button', { name: 'Save rule' }).click();
    await expect(page.getByText('Both genres')).toBeVisible();

    const rules = (await api('/rules')) as { name: string; conditions: unknown[] }[];
    expect(rules.find((rule) => rule.name === 'Both genres')?.conditions).toEqual([
      { type: 'genre_contains_all', value: ['Animation', 'Family'] },
    ]);
  });
});

test.describe('the routing journey', () => {
  test('create a rule, simulate, apply, then revert', async ({ page }) => {
    await api('/settings', {
      method: 'PUT',
      body: JSON.stringify({ settings: { global_dry_run: 'false' } }),
    });

    // --- create the rule through the editor -------------------------------
    await page.goto('/rules');
    await page.getByRole('button', { name: 'New Rule' }).click();

    await page.getByLabel('Rule name', { exact: true }).fill('Japanese animation');
    await page.getByLabel('Target category', { exact: true }).selectOption('anime');
    await addCondition(page, 'title_contains');
    await page.getByLabel('Title contains', { exact: true }).fill('akira, totoro, perfect blue');

    await page.getByRole('button', { name: 'Save rule' }).click();
    await expect(page.getByText('Japanese animation')).toBeVisible();

    // --- simulate ----------------------------------------------------------
    await page.goto('/simulation');
    await page.getByRole('button', { name: /run simulation/i }).click();

    // One move proposed for Akira, found by the control that selects it: every
    // row's explanation echoes the condition's values, so a row-level text
    // filter matches all of them.
    await expect(page.getByRole('checkbox', { name: 'Select the move for "Akira"' })).toHaveCount(
      1,
    );

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
    await page.getByRole('checkbox', { name: 'Select the move for "Akira"' }).check();
    await page.getByRole('button', { name: /apply selected/i }).click();

    await expect(page.locator('.banner-success')).toBeVisible();

    // It really reached the Arr, not just Routarr's own tables.
    const movies = (await (await fetch(`${ARR}/api/v3/movie`)).json()) as {
      title: string;
      rootFolderPath: string;
    }[];
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
    await confirmRevert.getByRole('button', { name: 'Put back' }).click();

    await expect(page.locator('.banner-success')).toBeVisible();

    // Back where it started, upstream as well as locally.
    const reverted = (await (await fetch(`${ARR}/api/v3/movie`)).json()) as {
      title: string;
      rootFolderPath: string;
    }[];
    expect(reverted.find((m) => m.title === 'Akira')?.rootFolderPath).toBe('/movies/standard');
  });
});

test.describe('guardrails are visible', () => {
  test('the shell states whether writing is possible', async ({ page }) => {
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

  test('arming automatic application warns before it can write', async ({ page }) => {
    // The two switches live in different sections, which is itself worth
    // exercising: the draft has to survive moving between them, or arming both
    // would be impossible in one visit.
    await page.goto('/settings#automation');
    await page.getByLabel('Automatic application').selectOption('true');

    // Automatic application alone is inert while dry-run holds, so it alone
    // must not warn.
    await expect(page.locator('.banner-warning')).toHaveCount(0);

    await page.getByRole('tab', { name: /guardrails/i }).click();
    await page.getByLabel('Global dry-run').selectOption('false');

    // Live mode, plus automatic application: the combination writes unattended,
    // and only the combination warns.
    await expect(page.locator('.banner-warning')).toHaveCount(2);
  });
});

test.describe('the API reference', () => {
  /**
   * Built from the contract the binary serves: an operation the screen drops,
   * a method it does not read, is one an outside application cannot find.
   */
  test('shows every operation the contract holds', async ({ page }) => {
    const contract = (await api('/openapi.json')) as { paths: Record<string, object> };
    const methods = ['get', 'post', 'put', 'patch', 'delete'];
    const expected = Object.entries(contract.paths)
      .flatMap(([path, item]) =>
        Object.keys(item)
          .filter((key) => methods.includes(key))
          .map((method) => `${method.toUpperCase()} ${path}`),
      )
      .sort();

    await openScreen(page, '/reference');
    await expect(page.locator('details.api-operation').first()).toBeVisible();

    // An operation's summary opens on its method badge, a schema's on its name.
    const shown = await page.locator('details.api-operation > summary').evaluateAll((summaries) =>
      summaries.flatMap((summary) => {
        const method = summary.querySelector('.badge')?.textContent?.trim();
        const path = summary.querySelector('code')?.textContent?.trim();
        return method ? [`${method} ${path}`] : [];
      }),
    );
    expect(shown.sort()).toEqual(expected);
  });
});

test.describe('reclassifying a whole library', () => {
  test('apply all reaches past the displayed list, in batches', async ({ page }) => {
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
    await expect(page.getByRole('checkbox', { name: 'Select the move for "Akira"' })).toHaveCount(
      1,
    );

    await page.getByRole('button', { name: /apply all/i }).click();
    await page.getByRole('alertdialog').getByRole('button', { name: 'Apply' }).click();

    // One banner, naming how many batches it took, not one dialog per batch.
    const banner = page.locator('.banner-success');
    await expect(banner).toBeVisible();
    await expect(banner).toContainText('3');

    // All three really moved, including the ones a single batch could not hold.
    const movies = (await (await fetch(`${ARR}/api/v3/movie`)).json()) as {
      title: string;
      rootFolderPath: string;
    }[];
    const moved = movies.filter((m) => m.rootFolderPath === '/movies/anime').map((m) => m.title);
    expect(moved.sort()).toEqual(['Akira', 'My Neighbor Totoro', 'Perfect Blue']);
    // Left unticked, no batch asked Radarr to move a file on disk.
    const edits = (await (await fetch(`${ARR}/__edits`)).json()) as { moveFiles: boolean }[];
    expect(edits.length).toBeGreaterThan(0);
    expect(edits.every((edit) => edit.moveFiles === false)).toBe(true);
  });

  test('moves the files on disk when the box asks for it', async ({ page }) => {
    await api('/settings', {
      method: 'PUT',
      body: JSON.stringify({ settings: { global_dry_run: 'false' } }),
    });
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

    await page.goto('/simulation');
    await page.getByRole('button', { name: /run simulation/i }).click();
    await expect(page.getByRole('checkbox', { name: 'Select the move for "Akira"' })).toBeChecked();
    await page.getByLabel(/Move the files on disk too/).check();
    await page.getByRole('button', { name: /apply selected/i }).click();

    await expect(page.locator('.banner-success')).toBeVisible();
    const edits = (await (await fetch(`${ARR}/__edits`)).json()) as {
      movieIds: number[];
      moveFiles: boolean;
    }[];
    expect(edits).toEqual([{ movieIds: [2], moveFiles: true }]);
  });

  /**
   * A destination that stopped answering is named in the one question Apply all
   * asks, before anything is written: a sleeping disk may wake on access and a
   * dead one will not, and only the reader can tell which.
   */
  test('apply all names a destination that is not answering before writing', async ({
    page,
    instanceId,
  }) => {
    // The sync below is followed by a simulation of its own, which would land
    // after the one run on screen and leave Apply all naming a superseded run.
    await api('/settings', {
      method: 'PUT',
      body: JSON.stringify({
        settings: { global_dry_run: 'false', auto_simulate_enabled: 'false' },
      }),
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
    await fetch(`${ARR}/__asleep?path=/movies/anime`);
    await apiWhenFree(`/instances/${instanceId}/sync`, { method: 'POST' });

    await page.goto('/simulation');
    await page.getByRole('button', { name: /run simulation/i }).click();
    await expect(page.getByRole('checkbox', { name: 'Select the move for "Akira"' })).toHaveCount(
      1,
    );
    await page.getByRole('button', { name: /apply all/i }).click();

    const dialog = page.getByRole('alertdialog');
    await expect(dialog).toContainText("'/movies/anime' is not answering");
    const writes = await writesDuring(page, () =>
      dialog.getByRole('button', { name: 'Cancel' }).click(),
    );
    expect(writes).toEqual([]);

    const movies = (await (await fetch(`${ARR}/api/v3/movie`)).json()) as {
      rootFolderPath: string;
    }[];
    expect(movies.filter((m) => m.rootFolderPath === '/movies/anime')).toHaveLength(0);
  });
});
