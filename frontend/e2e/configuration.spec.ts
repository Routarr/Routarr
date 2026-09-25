import { test, expect, api } from './fixtures';

/**
 * The screens the first suite left uncovered: root folders, overrides, and the
 * rule bundle round-trip.
 *
 * Each is a place where the interface writes something the user cannot easily
 * redo — a mapping, a human decision about one film, a whole rule set — so a
 * control wired to nothing costs more here than elsewhere.
 */

test.describe('root folders', () => {
  test('mapping a folder to a category sticks', async ({ page, instanceId }) => {
    expect(instanceId).toBeTruthy();
    await page.goto('/root-folders');
    await expect(page.getByRole('heading', { level: 1 })).toBeVisible();

    // The fixture maps both folders through the API; unmap one through the UI
    // and check the change survives a reload rather than only re-rendering.
    const row = page.locator('tbody tr').filter({ hasText: '/movies/anime' });
    await expect(row).toHaveCount(1);
    await row.locator('select').selectOption('');

    await expect(page.locator('.banner-success')).toBeVisible();
    await page.reload();
    await expect(
      page.locator('tbody tr').filter({ hasText: '/movies/anime' }).locator('select'),
    ).toHaveValue('');

    // Put it back, so the ordering of the suite does not matter.
    await page
      .locator('tbody tr')
      .filter({ hasText: '/movies/anime' })
      .locator('select')
      .selectOption('anime');
    await expect(page.locator('.banner-success')).toBeVisible();
  });

  test('renaming a category carries the rule that targets it', async ({ page, instanceId }) => {
    expect(instanceId).toBeTruthy();

    // A rule pointing at `anime`, so the rename has something to carry. The
    // backend transaction is covered by src/tests/categories.rs; what only a
    // browser can establish is that the control is wired to it at all, and that
    // the two screens agree afterwards.
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

    await page.goto('/root-folders');
    await expect(page.getByRole('heading', { level: 1 })).toBeVisible();

    const row = page.locator('tbody tr').filter({ hasText: 'anime' }).last();
    await row.getByRole('button', { name: /rename/i }).click();

    const dialog = page.locator('dialog[open]');
    await expect(dialog).toBeVisible();
    await dialog.locator('input').fill('japanese');
    await dialog.getByRole('button', { name: /save/i }).click();

    await expect(page.locator('.banner-success')).toBeVisible();
    // Two tables share this page, so rows rather than a bare tbody.
    await expect(page.locator('tr').filter({ hasText: 'japanese' }).first()).toBeVisible();

    // The folder mapping followed, on the same screen.
    await expect(
      page.locator('tr').filter({ hasText: '/movies/anime' }).locator('select'),
    ).toHaveValue('japanese');

    // And so did the rule, on another screen — the half a single-page
    // re-render could have faked.
    await page.goto('/rules');
    const rule = page.locator('tr').filter({ hasText: 'Japanese animation' });
    await expect(rule).toContainText('japanese');
    // The old name, gone from the row: the rename reached the rule's target.
    await expect(rule).not.toContainText(/\banime\b/);
  });

  test('an unmapped category is reported rather than left to guess', async ({
    page,
    instanceId,
  }) => {
    expect(instanceId).toBeTruthy();
    await api('/categories', { method: 'POST', body: JSON.stringify({ name: 'concerts' }) });

    await page.goto('/health');
    // Nothing points at `concerts`, and a rule targeting it would silently skip
    // every match — so diagnostics has to say so.
    await expect(page.getByText(/not mapped to any root folder/i).first()).toBeVisible();
  });
});

test.describe('overrides', () => {
  test('pinning a film outranks the rules and can be undone', async ({ page, instanceId }) => {
    expect(instanceId).toBeTruthy();
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

    await page.goto('/overrides');
    await page.getByRole('button', { name: /new override/i }).click();

    await page.getByPlaceholder(/search the library/i).fill('Akira');
    await page.getByRole('button', { name: /^search$/i }).click();
    await page.getByRole('button', { name: /select – akira/i }).click();

    await page.locator('.modal-content select').selectOption('standard');
    await page.getByRole('button', { name: /create override/i }).click();

    await expect(page.locator('tbody tr').filter({ hasText: 'Akira' })).toHaveCount(1);

    // The engine must now propose `standard`, not what the rule wanted.
    const decision = (await api('/media?search=Akira')) as { data: { id: string }[] };
    const explained = (await api(`/media/${decision.data[0].id}/explain`)) as {
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
    await page
      .getByRole('button', { name: /delete/i })
      .first()
      .click();
    const confirmation = page.locator('dialog[open]');
    await expect(confirmation).toContainText('Akira');
    await confirmation.getByRole('button', { name: 'Delete', exact: true }).click();
    await expect(page.locator('tbody tr').filter({ hasText: 'Akira' })).toHaveCount(0);

    const again = (await api(`/media/${decision.data[0].id}/explain`)) as {
      target_category: string;
    };
    expect(again.target_category).toBe('anime');
  });
});

test.describe('rule bundles', () => {
  test('a rule set survives an export and a re-import', async ({ page, instanceId }) => {
    expect(instanceId).toBeTruthy();
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
    // free of ids that mean nothing elsewhere.
    const bundle = JSON.parse(
      await (await import('node:fs/promises')).readFile(await file.path(), 'utf-8'),
    ) as { version: number; rules: { name: string; instance_ids: unknown }[] };
    expect(bundle.version).toBe(1);
    expect(bundle.rules.map((r) => r.name)).toContain('Round trip');
    expect(bundle.rules[0].instance_ids ?? null).toBeNull();

    // Wipe and restore through the interface.
    const rules = (await api('/rules')) as { id: string }[];
    for (const rule of rules) await api(`/rules/${rule.id}`, { method: 'DELETE' });

    await page.reload();
    await page.locator('input[type="file"]').setInputFiles(await file.path());
    // Three outcomes, so it is three buttons: a yes/no dialog would have to
    // map one of them onto Cancel.
    await page.locator('dialog[open]').getByRole('button', { name: 'Append' }).click();

    await expect(page.getByText('Round trip')).toBeVisible();
    const restored = (await api('/rules')) as { name: string; priority: number }[];
    expect(restored.find((r) => r.name === 'Round trip')?.priority).toBe(42);
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
    await page.goto('/settings#general');
    await expect(page.getByRole('heading', { level: 1 })).toBeVisible();

    await expect(page.getByLabel('Routarr API key')).toBeVisible();
    await expect(page.getByText('cannot be changed here')).toBeVisible();
    await expect(page.getByRole('button', { name: 'Regenerate' })).toHaveCount(0);
    await expect(page.getByRole('button', { name: 'Create a key' })).toHaveCount(0);
  });
});

test.describe('metadata sources', () => {
  /**
   * The priority list is the only control in Settings that is neither an input
   * nor a select: two buttons mutating an order that is saved as one string. A
   * button wired to nothing would look perfectly fine in a screenshot.
   */
  test('reordering the sources survives a save and a reload', async ({ page }) => {
    await page.goto('/settings#metadata');
    await expect(page.getByRole('heading', { level: 1 })).toBeVisible();

    const sources = page.locator('#setting-metadata_providers');
    await expect(sources.getByText('Radarr / Sonarr')).toBeVisible();

    await sources.getByLabel('Move up TMDb').click();
    await page.getByRole('button', { name: 'Save', exact: true }).click();
    await expect(page.locator('.banner-success')).toBeVisible();

    await page.reload();
    // First row of the active group, i.e. the source that now wins a contested
    // field. `.source-name` rather than `strong`: the list is two named groups
    // with a rank column, and the name is its own element.
    await expect(sources.locator('.source-name').first()).toHaveText('TMDb');

    // Put the shipped order back, so the ordering of the suite does not matter.
    await sources.getByLabel('Move down TMDb').click();
    await page.getByRole('button', { name: 'Save', exact: true }).click();
    await expect(page.locator('.banner-success')).toBeVisible();
  });
});

test.describe('metadata sources without a key', () => {
  /**
   * The stack runs with no TMDb, OMDb or TheTVDB key, which is exactly the
   * state this has to render honestly: those sources answer nothing, and the
   * list is a priority order, so showing them as ordinary would describe a
   * configuration the engine does not have. Only a real browser can tell that
   * a button is actually refused rather than merely styled as such.
   */
  test('a source that cannot answer is marked, and cannot be added', async ({ page }) => {
    // Addressed by its section: the settings are grouped into tabs, and the
    // hash is what makes one of them linkable.
    await page.goto('/settings#metadata');
    const sources = page.locator('#setting-metadata_providers');
    await expect(sources).toBeVisible();

    // TMDb ships enabled by default and has no key here: it stays in the list,
    // in its position, and says so in words rather than only by being greyed.
    const tmdb = sources.locator('.source-row').filter({ hasText: 'TMDb' });
    await expect(tmdb.locator('.badge-warning')).toHaveText('inactive');

    // OMDb is not in the list and cannot be put in it until a key is given —
    // and the field that accepts one is in its own row, naming the variable
    // that is the other way to supply it. Stated three blocks further down,
    // this took a scroll, a save, a scroll back and a second save.
    const omdb = sources.locator('.source-row').filter({ hasText: 'OMDb' });
    const enable = omdb.getByRole('button', { name: 'Enable' });
    await expect(enable).toBeDisabled();
    await expect(omdb.locator('input')).toHaveAttribute('placeholder', /OMDB_API_KEY/);

    // A key typed but not yet saved counts: refusing the click then would send
    // the reader back for a save they cannot see the need for.
    await omdb.locator('input').fill('a-key');
    await expect(enable).toBeEnabled();

    // AniList needs no key at all, so nothing stands in the way of enabling it.
    const anilist = sources.locator('.source-row').filter({ hasText: 'AniList' });
    await expect(anilist.getByRole('button', { name: 'Enable' })).toBeEnabled();
  });
});

test.describe('backups', () => {
  /**
   * A backup is the one feature whose failure is invisible until the day it
   * matters. This drives the whole loop in a real browser — take one, see it
   * listed, delete it — against the real binary writing a real archive.
   */
  test('taking a backup produces one that is listed and removable', async ({ page }) => {
    await page.goto('/settings#maintenance');
    await expect(page.getByRole('heading', { level: 1 })).toBeVisible();

    await page.getByRole('button', { name: 'Back up now' }).click();

    const entry = page.locator('.mono').filter({ hasText: /^routarr-backup-/ });
    await expect(entry.first()).toBeVisible();

    // Removable, and the list reflects it without a reload.
    const name = (await entry.first().textContent())!.trim();
    await page.getByLabel(`Delete ${name}`).click();
    // Deleting is the one irreversible half of the pair: a restore is staged
    // and undone by not restarting, a deleted archive is the only copy.
    await page.locator('dialog[open]').getByRole('button', { name: 'Delete', exact: true }).click();
    await expect(page.locator('.mono').filter({ hasText: name })).toHaveCount(0);
  });
});

test.describe('theme', () => {
  /**
   * The palette is a set of CSS custom properties, so "the light theme works"
   * means the browser actually resolved different values — something no unit
   * test can see. It also guards the split between the accent used as a *fill*
   * (unchanged, dark text on orange) and as *text*, which has to darken or it
   * reads at about 1.9:1 on white.
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
     * saving a value that is already the stored one is not possible — and does
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
    // The fill is shared between the two themes; only the text form darkens.
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
   * The preview answers "what would this change". Nothing answered "what must
   * it not change" — and rules are first-match-by-priority, so inserting one
   * rebalances every rule below it. A pinned case is the only thing that
   * notices when the routing nobody was watching moves.
   */
  test('a pinned decision fails once a rule stops producing it', async ({ page, instanceId }) => {
    expect(instanceId).toBeTruthy();

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

    // Pinned from the explanation panel, which already holds the whole answer —
    // that is what makes it one click rather than a form.
    await page.goto('/media');
    await page
      .getByRole('button', { name: /^Why\?/ })
      .first()
      .click();
    const panel = page.locator('dialog[open]');
    await panel.getByRole('button', { name: 'Pin as test' }).click();
    await expect(panel.getByRole('button', { name: 'Pinned' })).toBeVisible();
    await page.keyboard.press('Escape');

    await page.goto('/rules/tests');
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
