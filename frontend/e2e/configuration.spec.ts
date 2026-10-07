import { test, expect, api, openScreen, proveWithKey, writesDuring, API, ARR } from './fixtures';

/**
 * The screens that configure the routing: root folders, exceptions, the rule
 * bundle round-trip, the settings and the instance form.
 *
 * Each is a place where the interface writes something the user cannot easily
 * redo (a mapping, a human decision about one film, a whole rule set), so a
 * control wired to nothing costs more here than elsewhere.
 */

test.describe('root folders', () => {
  test('mapping a folder to a category sticks', async ({ page }) => {
    await page.goto('/categories');

    // The fixture maps both folders through the API. One is unmapped through the
    // UI, and the change has to survive a reload rather than only re-render.
    const folder = page.getByRole('combobox', { name: 'Category for /movies/anime' });
    await folder.selectOption('');
    await page.getByRole('button', { name: 'Save – /movies/anime' }).click();

    await expect(page.locator('.banner-success')).toBeVisible();
    await page.reload();
    await expect(folder).toHaveValue('');
  });

  test('renaming a category carries the rule that targets it', async ({ page }) => {
    // A rule pointing at `anime`, so the rename has something to carry. The
    // backend transaction is covered by backend/src/tests/categories.rs. What
    // only a browser can establish is that the control is wired to it at all,
    // and that the two screens agree afterwards.
    await api('/rules', {
      method: 'POST',
      body: JSON.stringify({
        name: 'Japanese animation',
        target_category: 'anime',
        media_type: 'both',
        match_mode: 'all',
        priority: 10,
        enabled: true,
        conditions: [{ type: 'genre_contains', value: ['Animation'] }],
      }),
    });

    await page.goto('/categories');
    await page.getByRole('button', { name: 'Rename category – anime' }).click();

    const dialog = page.getByRole('dialog');
    await expect(dialog).toBeVisible();
    await dialog.getByLabel('Name', { exact: true }).fill('japanese');
    await dialog.getByRole('button', { name: /save/i }).click();

    await expect(page.locator('.banner-success')).toBeVisible();
    await expect(page.getByRole('button', { name: 'Rename category – japanese' })).toBeVisible();

    // The folder mapping followed, on the same screen.
    await expect(page.getByRole('combobox', { name: 'Category for /movies/anime' })).toHaveValue(
      'japanese',
    );

    // And so did the rule, on another screen: the half a single-page re-render
    // could have faked.
    await page.goto('/rules');
    const rule = page.getByRole('row').filter({ hasText: 'Japanese animation' });
    await expect(rule).toContainText('japanese');
    // The old name, gone from the row: the rename reached the rule's target.
    await expect(rule).not.toContainText(/\banime\b/);
  });

  test('an unmapped category is reported rather than left to guess', async ({ page }) => {
    await api('/categories', { method: 'POST', body: JSON.stringify({ name: 'concerts' }) });

    await page.goto('/diagnostics');
    // Nothing points at `concerts`, and a rule targeting it would silently skip
    // every match, so diagnostics has to say so. Counted, since the reset maps
    // every other category: one reported is this one.
    await expect(
      page.getByText('Categories not mapped to any root folder: 1.').first(),
    ).toBeVisible();
  });
});

test.describe('exceptions', () => {
  test('pinning a film outranks the rules and can be undone', async ({ page }) => {
    // A rule that would send Akira to anime, so the override has something to
    // outrank rather than merely agreeing with.
    await api('/rules', {
      method: 'POST',
      body: JSON.stringify({
        name: 'Anime by title',
        target_category: 'anime',
        media_type: 'movie',
        priority: 10,
        enabled: true,
        condition_logic: 'any',
        conditions: [{ type: 'title_contains', value: ['akira'] }],
        exclusions: [],
      }),
    });

    await page.goto('/exceptions');
    await page.getByRole('button', { name: /new exception/i }).click();

    await page.getByPlaceholder(/search the library/i).fill('Akira');
    await page.getByRole('button', { name: /^search$/i }).click();
    await page.getByRole('button', { name: /select – akira/i }).click();

    await page.getByLabel('Force category for "Akira"').selectOption('standard');
    await page.getByRole('button', { name: /create exception/i }).click();

    await expect(page.getByRole('row').filter({ hasText: 'Akira' })).toHaveCount(1);

    // The engine must now propose `standard`, not what the rule wanted.
    const found = (await api('/media?search=Akira')) as { data: { id: string }[] };
    const [akira] = found.data;
    if (!akira) throw new Error('Akira is not in the library');
    const explained = (await api(`/media/${akira.id}/explain`)) as {
      target_category: string;
      override_category: string | null;
    };
    expect(explained.target_category).toBe('standard');
    // Not merely "the answer happens to be standard": the explanation has to say
    // a human decided it, or the screen cannot tell the user why.
    expect(explained.override_category).toBe('standard');

    // Removing it hands the decision back to the rules. The confirmation is an
    // in-page `<dialog>`, so this asserts the question rather than installing
    // `page.on('dialog')`, a blanket handler that cannot tell the right
    // question from any question. Addressed by its accessible name, which the
    // button has because it is destructive and icon-only.
    await page.getByRole('button', { name: 'Delete – Akira' }).click();
    const confirmation = page.getByRole('dialog');
    await expect(confirmation).toContainText('Akira');
    await confirmation.getByRole('button', { name: 'Delete', exact: true }).click();
    await expect(page.getByRole('row').filter({ hasText: 'Akira' })).toHaveCount(0);

    const again = (await api(`/media/${akira.id}/explain`)) as {
      target_category: string;
    };
    expect(again.target_category).toBe('anime');
  });
});

test.describe('rule bundles', () => {
  test('a rule set survives an export and a re-import', async ({ page }) => {
    await api('/rules', {
      method: 'POST',
      body: JSON.stringify({
        name: 'Round trip',
        target_category: 'anime',
        media_type: 'movie',
        priority: 42,
        enabled: true,
        condition_logic: 'all',
        conditions: [{ type: 'title_contains', value: ['totoro'] }],
        exclusions: [],
      }),
    });

    await page.goto('/rules');
    const download = page.waitForEvent('download');
    await page.getByRole('button', { name: /^export$/i }).click();
    const file = await download;

    // The bundle is what a user versions in git, so it has to be readable and
    // free of ids that mean nothing elsewhere: a scope travels by name.
    const bundle = JSON.parse(
      await (await import('node:fs/promises')).readFile(await file.path(), 'utf-8'),
    ) as {
      version: number;
      rules: { name: string; instance_ids: unknown; instance_names?: unknown }[];
    };
    expect(bundle.version).toBe(2);
    const roundTrip = bundle.rules.find((r) => r.name === 'Round trip');
    expect(roundTrip).toBeDefined();
    expect(roundTrip?.instance_ids ?? null).toBeNull();
    expect(roundTrip?.instance_names ?? null).toBeNull();

    // Wiped through the API, restored through the interface beside a rule
    // written since: Append keeps it, and Replace would delete it.
    const rules = (await api('/rules')) as { id: string }[];
    for (const rule of rules) await api(`/rules/${rule.id}`, { method: 'DELETE' });
    await api('/rules', {
      method: 'POST',
      body: JSON.stringify({
        name: 'Written since',
        target_category: 'anime',
        media_type: 'movie',
        priority: 7,
        enabled: true,
        condition_logic: 'any',
        conditions: [{ type: 'title_contains', value: ['akira'] }],
        exclusions: [],
      }),
    });

    await page.reload();
    const chooser = page.waitForEvent('filechooser');
    await page.getByRole('button', { name: 'Import' }).click();
    await (await chooser).setFiles(await file.path());
    // Three outcomes, so it is three buttons: a yes/no dialog would have to
    // map one of them onto Cancel.
    await page.getByRole('dialog').getByRole('button', { name: 'Append' }).click();

    await expect(page.getByText('Round trip')).toBeVisible();
    const restored = (await api('/rules')) as { name: string; priority: number }[];
    expect(restored.find((r) => r.name === 'Round trip')?.priority).toBe(42);
    expect(restored.map((r) => r.name)).toContain('Written since');
  });
});

test.describe('the API key card', () => {
  /**
   * The harness runs with `ROUTARR_API_KEY` set, which is exactly the pinned
   * case: the variable wins over anything the interface could mint, so the
   * controls that would mint one must not be there. A button that appears and
   * then reports a conflict is worse than an absent one, and only a real
   * browser against a real server can tell which of the two shipped.
   */
  test('offers no rotation while the environment sets the key', async ({ page }) => {
    await page.goto('/settings#security');
    await expect(page.getByRole('heading', { level: 1 })).toBeVisible();

    await expect(page.getByRole('heading', { name: 'Routarr API key' })).toBeVisible();
    await expect(page.getByText('cannot be changed here')).toBeVisible();
    await expect(page.getByRole('button', { name: 'Regenerate' })).toHaveCount(0);
    await expect(page.getByRole('button', { name: 'Create a key' })).toHaveCount(0);
  });
});

test.describe('application keys', () => {
  /**
   * The token is read off the screen once and used as another application
   * would: what the key was given answers, what it was not is refused, and a
   * revoked key opens nothing.
   */
  test('a key made on the screen reads, is held to its scopes and dies revoked', async ({
    page,
  }) => {
    await openScreen(page, '/applications');
    await page.getByRole('button', { name: 'New key' }).click();
    const dialog = page.getByRole('dialog');
    await dialog.getByLabel('Name').fill('n8n');
    // Ticked through the screen: a checkbox wired to nothing would send a key
    // that cannot operate, and the simulation below would answer 403.
    await dialog.getByRole('checkbox', { name: /^operate/i }).check();
    await dialog.getByRole('button', { name: 'Create a key' }).click();
    await proveWithKey(page);

    // Read as the reader reads it, the one token on the page.
    const token = ((await page.getByText(/^rtr_\S+$/).textContent()) ?? '').trim();
    expect(token).toMatch(/^rtr_/);
    const call = (path: string, method = 'GET') =>
      fetch(`${API}${path}`, {
        method,
        headers: { 'x-api-key': token, 'content-type': 'application/json' },
        body: method === 'GET' ? undefined : '{}',
      });

    expect((await call('/status')).status).toBe(200);
    expect((await call('/simulate', 'POST')).status).not.toBe(403);
    expect((await call('/overrides', 'POST')).status).toBe(403);
    expect((await call('/settings')).status).toBe(403);

    await page.getByRole('button', { name: 'Revoke – n8n' }).click();
    await page.getByRole('dialog').getByRole('button', { name: 'Revoke' }).click();
    await expect(page.getByText('No application has a key yet.')).toBeVisible();
    expect((await call('/status')).status).toBe(401);
  });
});

/**
 * Store a source list, keyed sources included. The API refuses to add a source
 * with no key, so each goes in with one, removed right after: how an
 * installation ends up listing a source it has no key for.
 */
async function listSources(order: string): Promise<void> {
  const keyed = ['tmdb', 'omdb', 'tvdb'].filter((id) => order.split(',').includes(id));
  const keys = (value: string) => Object.fromEntries(keyed.map((id) => [`${id}_api_key`, value]));
  await api('/settings', {
    method: 'PUT',
    body: JSON.stringify({ settings: { metadata_providers: order, ...keys('e2e-removed-below') } }),
  });
  if (keyed.length > 0) {
    await api('/settings', { method: 'PUT', body: JSON.stringify({ settings: keys('') }) });
  }
}

test.describe('metadata sources', () => {
  // The suite shares one server: the shipped list goes back after each test,
  // whatever the test left.
  test.afterEach(() => listSources('arr'));

  /**
   * The priority list is the only control among the settings that is neither an input
   * nor a select: two buttons mutating an order that is saved as one string. A
   * button wired to nothing would look perfectly fine in a screenshot.
   */
  test('reordering the sources survives a save and a reload', async ({ page }) => {
    // TMDB has no key here and answers nothing, but a listed source is ordered
    // all the same, and it is the one every stack knows.
    await listSources('arr,tmdb');
    await page.goto('/sources');
    await expect(page.getByRole('heading', { level: 1 })).toBeVisible();

    const sources = page.locator('#setting-metadata_providers');
    await expect(sources.getByText('Radarr / Sonarr')).toBeVisible();

    await sources.getByLabel('Move up – TMDB').click();
    await page.getByRole('button', { name: 'Save', exact: true }).click();
    await expect(page.locator('.banner-success')).toBeVisible();

    await page.reload();
    // First row of the active group, i.e. the source that now wins a contested
    // field. `.source-name` rather than `strong`: the list is two named groups
    // with a rank column, and the name is its own element.
    await expect(sources.locator('.source-name').first()).toHaveText('TMDB');
  });
});

test.describe('metadata sources without a key', () => {
  test.afterEach(() => listSources('arr'));

  /**
   * A fresh installation has configured nothing yet, so nothing on screen
   * should call it a fault: TMDB waits in the inactive group for a key, and
   * the diagnostics say nothing about it.
   */
  test('a fresh stack lists the Arr alone and raises no key warning', async ({ page }) => {
    await page.goto('/sources');
    const sources = page.locator('#setting-metadata_providers');
    await expect(sources.getByRole('button', { name: 'Enable – TMDB' })).toBeDisabled();
    await expect(sources.getByLabel('TMDB', { exact: true })).toHaveAttribute(
      'placeholder',
      /TMDB_API_KEY/,
    );

    // The source rows arrive with the warnings, so the list is loaded before
    // its silence is read.
    await page.goto('/diagnostics');
    await expect(page.locator('.source-name')).toHaveText(['Radarr / Sonarr']);
    await expect(page.getByText(/TMDB is in the source list/)).toHaveCount(0);

    // The same reading finds the warning once TMDB is listed without its key.
    await listSources('arr,tmdb');
    await page.reload();
    await expect(page.getByText(/TMDB is in the source list/)).toBeVisible();
  });

  /**
   * The stack runs with no TMDB, OMDb or TheTVDB key, which is exactly the
   * state this has to render honestly: those sources answer nothing, and the
   * list is a priority order, so showing them as ordinary would describe a
   * configuration the engine does not have. Only a real browser can tell that
   * a button is actually refused rather than merely styled as such.
   */
  test('a source that cannot answer is marked, and cannot be added', async ({ page }) => {
    await listSources('arr,tmdb');
    // Addressed by its section: the settings are grouped into tabs, and the
    // hash is what makes one of them linkable.
    await page.goto('/sources');
    const sources = page.locator('#setting-metadata_providers');
    await expect(sources).toBeVisible();

    // TMDB is listed and has no key here: it stays in the list, in its
    // position, and says so in words rather than only by being greyed.
    const tmdb = sources.locator('.source-row').filter({ hasText: 'TMDB' });
    await expect(tmdb.getByText('inactive', { exact: true })).toBeVisible();

    // OMDb is not in the list and cannot be put in it until a key is given.
    // The field that accepts one is in its own row, so no scroll and no save
    // stand between the key and the button, and it names the variable that is
    // the other way to supply it.
    const enable = sources.getByRole('button', { name: 'Enable – OMDb' });
    const key = sources.getByLabel('OMDb', { exact: true });
    await expect(enable).toBeDisabled();
    await expect(key).toHaveAttribute('placeholder', /OMDB_API_KEY/);

    // A key typed but not yet saved counts: refusing the click then would send
    // the reader back for a save they cannot see the need for.
    await key.fill('a-key');
    await expect(enable).toBeEnabled();

    // AniList needs no key at all, so nothing stands in the way of enabling it.
    await expect(sources.getByRole('button', { name: 'Enable – AniList' })).toBeEnabled();
  });
});

test.describe('backups', () => {
  /**
   * A backup is the one feature whose failure is invisible until the day it
   * matters. This drives the whole loop in a real browser (take one, see it
   * listed, delete it) against the real binary writing a real archive.
   */
  test('taking a backup produces one that is listed and removable', async ({ page }) => {
    // The archives already there, which another spec's Back up now leaves: the
    // one this click makes is the one name that was not.
    const names = async () =>
      ((await api('/backups')) as { backups: { name: string }[] }).backups.map((b) => b.name);
    const before = new Set(await names());
    await openScreen(page, '/settings#maintenance');

    await page.getByRole('button', { name: 'Back up now' }).click();

    await expect.poll(async () => (await names()).filter((n) => !before.has(n))).toHaveLength(1);
    const name = (await names()).find((n) => !before.has(n))!;
    await expect(page.getByText(name)).toBeVisible();

    // Removable, and the list reflects it without a reload.
    await page.getByLabel(`Delete – ${name}`).click();
    // Deleting is the one irreversible half of the pair: a restore is staged
    // and undone by not restarting, a deleted archive is the only copy.
    await page.getByRole('dialog').getByRole('button', { name: 'Delete', exact: true }).click();
    await expect(page.getByText(name)).toHaveCount(0);
  });
});

test.describe('theme', () => {
  /**
   * The palette is a set of CSS custom properties, so "the light theme works"
   * means the browser actually resolved different values, which no unit test
   * can see. It also guards the split between the accent used as a *fill*
   * (unchanged, dark text on orange) and as *text*, which has to darken or it
   * falls short of AA contrast on white.
   */
  test('choosing the light theme repaints the interface', async ({ page }) => {
    const read = () =>
      page.evaluate(() => {
        const style = getComputedStyle(document.documentElement);
        return {
          background: style.getPropertyValue('--bg-base').trim(),
          text: style.getPropertyValue('--text-primary').trim(),
          accentFill: style.getPropertyValue('--accent-primary').trim(),
          accentText: style.getPropertyValue('--accent-strong').trim(),
          marker: document.documentElement.dataset.theme ?? 'auto',
        };
      });

    await page.goto('/settings#general');
    await expect(page.getByRole('heading', { level: 1 })).toBeVisible();

    /**
     * The save bar only exists while the draft differs from what is stored, so
     * saving a value that is already the stored one is not possible, and does
     * not need to be. `save` handles both: it presses the button when there is
     * something to press, and otherwise leaves the settings as they already
     * are, which is what was wanted anyway.
     */
    const save = async () => {
      const button = page.getByRole('button', { name: 'Save', exact: true });
      if (await button.isVisible()) {
        await button.click();
        await expect(page.locator('.banner-success')).toBeVisible();
      }
    };

    // Set explicitly rather than assumed: `auto` resolves to whatever the
    // browser running the suite prefers, which is not a fixed starting point.
    await page.getByLabel('Theme').selectOption('dark');
    await save();
    const dark = await read();

    await page.getByLabel('Theme').selectOption('light');
    await save();

    await page.reload();
    await expect(page.getByRole('heading', { level: 1 })).toBeVisible();
    const light = await read();

    expect(light.marker).toBe('light');
    expect(light.background).not.toBe(dark.background);
    expect(light.text).not.toBe(dark.text);
    // The fill is shared between the two themes, and only the text form darkens.
    expect(light.accentFill).toBe(dark.accentFill);
    expect(light.accentText).not.toBe(dark.accentText);

    // Put the shipped theme back, so the ordering of the suite does not matter.
    await page.getByLabel('Theme').selectOption('dark');
    await page.getByRole('button', { name: 'Save', exact: true }).click();
    await expect(page.locator('.banner-success')).toBeVisible();
  });
});

test.describe('rule tests', () => {
  /**
   * The feature's whole claim in one journey: pin a decision, change the rules
   * under it, and be told.
   *
   * The preview answers "what would this change", not "what must it not
   * change". Rules are first-match-by-priority, so inserting one rebalances
   * every rule below it, and a pinned case is the only thing that notices when
   * the routing nobody was watching moves.
   */
  test('a pinned decision fails once a rule stops producing it', async ({ page }) => {
    await api('/rules', {
      method: 'POST',
      body: JSON.stringify({
        name: 'Everything japanese',
        target_category: 'anime',
        media_type: 'movie',
        priority: 10,
        enabled: true,
        condition_logic: 'any',
        conditions: [{ type: 'title_contains', value: ['akira'] }],
        exclusions: [],
      }),
    });

    // Pinned from the explanation panel, which already holds the whole answer:
    // that is what makes it one click rather than a form.
    await page.goto('/library');
    await page
      .getByRole('button', { name: /^Why\?/ })
      .first()
      .click();
    const panel = page.getByRole('dialog');
    await panel.getByRole('button', { name: 'Pin as test' }).click();
    await expect(panel.getByRole('button', { name: 'Pinned' })).toBeVisible();
    await page.keyboard.press('Escape');

    await page.goto('/rule-tests');
    await page.getByRole('button', { name: 'Run tests' }).click();
    await expect(page.locator('.banner-success')).toBeVisible();

    // Now take the rule away. The case has to notice, and say where the title
    // goes instead rather than only that something moved.
    const rules = (await api('/rules')) as { id: string; name: string }[];
    const target = rules.find((r) => r.name === 'Everything japanese')!;
    await api(`/rules/${target.id}`, { method: 'DELETE' });

    await page.reload();
    await page.getByRole('button', { name: 'Run tests' }).click();
    await expect(page.getByRole('alert')).toBeVisible();
    await expect(page.getByText(/now goes to/)).toBeVisible();
  });
});

test.describe('the instance form', () => {
  /**
   * The values are tried before anything is saved: against the stand-in Arr,
   * and against a port nothing listens on, where the reason names what to
   * change instead of the transport's own words.
   */
  test('tries the typed values and says what to change when they fail', async ({ page }) => {
    await page.goto('/instances');
    await page.getByRole('button', { name: 'Add instance' }).click();
    const dialog = page.getByRole('dialog');
    const address = dialog.getByLabel('Base URL', { exact: true });
    const test = dialog.getByRole('button', { name: 'Test connectivity' });

    await address.fill(ARR);
    await dialog.getByLabel('API key', { exact: true }).fill('e2e-key');
    await test.click();
    await expect(dialog.getByRole('status')).toContainText('connected');

    await address.fill('http://127.0.0.1:1');
    await test.click();
    await expect(dialog.getByRole('alert')).toContainText('Nothing answers at http://127.0.0.1:1');
    await expect(dialog.getByRole('status')).toBeEmpty();
  });
});

test.describe('a deletion asked about', () => {
  /**
   * Cancel is the answer every guarded action relies on. Pressed, the rule
   * stays on screen and on the server.
   */
  test('cancelling a deletion leaves the row where it was', async ({ page }) => {
    await api('/rules', {
      method: 'POST',
      body: JSON.stringify({
        name: 'Stays put',
        target_category: 'anime',
        media_type: 'movie',
        priority: 10,
        enabled: true,
        condition_logic: 'all',
        conditions: [{ type: 'title_contains', value: ['totoro'] }],
        exclusions: [],
      }),
    });

    await page.goto('/rules');
    await page.getByRole('button', { name: 'Actions – Stays put' }).click();
    await page.getByRole('menuitem', { name: 'Delete' }).click();
    const dialog = page.getByRole('dialog');
    await expect(dialog).toContainText('Delete the rule "Stays put"?');
    const writes = await writesDuring(page, () =>
      dialog.getByRole('button', { name: 'Cancel' }).click(),
    );
    expect(writes).toEqual([]);

    await expect(dialog).toHaveCount(0);
    await expect(page.getByRole('button', { name: 'Actions – Stays put' })).toBeVisible();
    const rules = (await api('/rules')) as { name: string }[];
    expect(rules.map((rule) => rule.name)).toContain('Stays put');
  });
});
