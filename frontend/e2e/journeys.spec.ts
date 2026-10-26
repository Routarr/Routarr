import type { Page } from '@playwright/test';

import { test, expect, api, openScreen, screenShown, ARR } from './fixtures';
import type { RuleDraft } from '../src/api/types';

/**
 * The journeys a user walks, against the real binary: a dead route, a control wired to nothing
 * or a field renamed on one side only shows here and in no other test.
 */

test.describe('navigation', () => {
  test('every sidebar entry reaches a page that renders', async ({ page }) => {
    // Counted by hand: a sidebar link loads no page, so networkidle answers before the loads.
    let inFlight = 0;
    page.on('request', () => (inFlight += 1));
    page.on('requestfinished', () => (inFlight -= 1));
    page.on('requestfailed', () => (inFlight -= 1));
    await page.goto('/');

    // Read from the sidebar, so a link added without a page fails this test.
    const links = page.getByRole('navigation', { name: 'Main navigation' }).getByRole('link');
    const count = await links.count();
    expect(count).toBeGreaterThan(5);

    for (let index = 0; index < count; index += 1) {
      const link = links.nth(index);
      const label = (await link.innerText()).trim();
      await link.click();

      // The heading draws before the loads, and a load answering 500 shows its banner after.
      await screenShown(page, label);
      await expect.poll(() => inFlight, { message: `${label} never settles` }).toBe(0);
      await expect(page.getByRole('alert'), `${label} shows an error`).toHaveCount(0);
    }
  });

  test('the chrome speaks the configured language', async ({ page }) => {
    // In English most keys read as their translation, so only another language proves this.
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
   * The server answers 200 for any path, so a mistyped one is the client's to report. The
   * dashboard in its place would be a screen nobody asked for.
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

    // The header's own link, where every action is, and not the sidebar's.
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
   * The browser has moved by the time Back is heard, so only a browser shows that Cancel puts
   * the address back with the draft still open.
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
   * The library's values reach the picker, choosing one stores its exact spelling, and a second
   * one is an alternative rather than a second requirement.
   */
  test('a genre rule is built from the library, not from memory', async ({ page }) => {
    await page.goto('/rules');
    await page.getByRole('button', { name: 'New Rule' }).click();

    await page.getByLabel('Rule name', { exact: true }).fill('Animated');
    await page.getByLabel('Target category', { exact: true }).selectOption('anime');
    await addCondition(page, 'genre_contains');

    // Named exactly: the quantifier selector beside it carries the same words.
    const picker = page.getByRole('combobox', { name: 'Genre contains', exact: true });
    await picker.click();
    await expect(page.getByRole('option', { name: 'Animation' })).toBeVisible();

    // Typed without its capital: the spelling is not the user's to remember.
    await picker.fill('anim');
    await page.getByRole('option', { name: 'Animation' }).click();

    await page.getByRole('button', { name: 'Save rule' }).click();
    const table = page.getByRole('table', { name: 'Rules' });
    await expect(table.getByText('Animated', { exact: true })).toBeVisible();

    // What was stored is the library's spelling, in the multi-value shape.
    const rules = (await api('/rules')) as { name: string; conditions: unknown[] }[];
    const saved = rules.find((rule) => rule.name === 'Animated');
    expect(saved?.conditions).toEqual([{ type: 'genre_contains', value: ['Animation'] }]);
  });
});

test.describe('the routing journey', () => {
  test('create a rule, simulate, apply, then revert', async ({ page }) => {
    await api('/settings', {
      method: 'PUT',
      body: JSON.stringify({ settings: { global_dry_run: 'false' } }),
    });

    await page.goto('/rules');
    await page.getByRole('button', { name: 'New Rule' }).click();

    await page.getByLabel('Rule name', { exact: true }).fill('Japanese animation');
    await page.getByLabel('Target category', { exact: true }).selectOption('anime');
    await addCondition(page, 'title_contains');
    await page.getByLabel('Title contains', { exact: true }).fill('akira, totoro, perfect blue');

    await page.getByRole('button', { name: 'Save rule' }).click();
    await expect(page.getByText('Japanese animation')).toBeVisible();

    await page.goto('/simulation');
    await page.getByRole('button', { name: /run simulation/i }).click();

    // Found by its checkbox, since every row's explanation echoes the condition's values.
    await expect(page.getByRole('checkbox', { name: 'Select the move for "Akira"' })).toHaveCount(
      1,
    );

    // The explanation is on screen beside the move, never a click away.
    await expect(page.getByText(/Title contains .*found \[Akira\]/)).toBeVisible();
    await expect(page.getByRole('cell', { name: '/movies/anime' }).first()).toBeVisible();

    // Below the confirmation threshold the apply asks nothing, so the banner follows the click.
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

    await page.goto('/history');
    await page.getByRole('button', { name: 'Revert – Akira' }).click();

    // One dialog asks and carries the "move the files too" choice: no second prompt to cancel.
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

test.describe('the templates', () => {
  test('a template from the gallery lands as rules the table lists', async ({ page }) => {
    await page.goto('/rules');
    await page.getByRole('button', { name: 'Add – Anime' }).click();

    const dialog = page.getByRole('dialog', { name: 'Anime' });
    await expect(dialog.getByLabel('Category')).toHaveValue('anime');
    // The preview answers before anything is stored.
    await expect(dialog.getByRole('heading', { name: 'What it would do' })).toBeVisible();
    await expect(dialog.getByRole('status')).toHaveText(/Titles it recognises|no title/);
    await dialog.getByRole('button', { name: 'Add' }).click();

    await expect(page.getByText('Template "Anime" added. Rules created: 2')).toBeVisible();
    const table = page.getByRole('table', { name: 'Rules' });
    await expect(table.getByText('Anime series', { exact: true })).toBeVisible();
    await expect(table.getByText('Anime films', { exact: true })).toBeVisible();
    await expect(page.getByRole('button', { name: 'Replace – Anime' })).toBeVisible();
  });
});

test.describe('folder suggestions', () => {
  /** A folder named for its category is mapped in one click, and leads on to its template. */
  test('a folder named like a category is mapped from its suggestion', async ({
    page,
    instanceId,
  }) => {
    await api(`/root-folders/rf-${instanceId}-2/category`, {
      method: 'PUT',
      body: JSON.stringify({ category: null }),
    });
    await openScreen(page, '/categories');

    const card = page.getByRole('region', { name: 'Suggested from folder names' });
    await expect(card.getByText('Named like the category anime.')).toBeVisible();
    await card.getByRole('button', { name: 'Map – /movies/anime' }).click();

    await expect(page.getByText('/movies/anime is mapped to anime.')).toBeVisible();
    await expect(page.getByLabel('Category for /movies/anime', { exact: true })).toHaveValue(
      'anime',
    );
    await card.getByRole('link', { name: 'Add the Anime rules – /movies/anime' }).click();
    const dialog = page.getByRole('dialog', { name: 'Anime' });
    await expect(dialog.getByLabel('Category')).toHaveValue('anime');
  });
});

test.describe('guardrails are visible', () => {
  test('the shell states whether writing is possible', async ({ page }) => {
    await page.goto('/');
    // Top bar only: the dashboard links a dry-run too. The chip's name holds the full sentence.
    const mode = page.locator('.topbar .mode-chip');
    await expect(mode).toHaveAccessibleName(/dry-run/i);
    await expect(page.locator('.topbar .mode-dot')).toHaveClass(/is-held/);

    await api('/settings', {
      method: 'PUT',
      body: JSON.stringify({ settings: { global_dry_run: 'false' } }),
    });
    await page.reload();
    await expect(mode).toHaveAccessibleName(/live mode/i);
    // Live never paints danger. Compared as resolved colours: a hex token never equals an rgb().
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
});

test.describe('the API reference', () => {
  /**
   * An operation the screen drops is one an outside application cannot find. An old path is
   * listed once, in the deprecated paths, beside the one that replaced it.
   */
  test('shows every operation the contract holds', async ({ page }) => {
    const contract = (await api('/openapi.json')) as {
      paths: Record<string, Record<string, { deprecated?: boolean }>>;
    };
    const methods = ['get', 'post', 'put', 'patch', 'delete'];
    const listed = (deprecated: boolean) =>
      Object.entries(contract.paths)
        .flatMap(([path, item]) =>
          Object.entries(item)
            .filter(([key, op]) => methods.includes(key) && (op.deprecated === true) === deprecated)
            .map(([method]) => `${method.toUpperCase()} ${path}`),
        )
        .sort();
    const expected = listed(false);

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
    const old = page
      .getByRole('table', { name: 'Deprecated paths' })
      .locator('tbody tr td:first-child');
    expect((await old.allTextContents()).map((cell) => cell.trim()).sort()).toEqual(listed(true));
    expect(shown.sort()).toEqual(expected);
  });
});

test.describe('reclassifying a whole library', () => {
  test('apply all reaches past the displayed list, in batches', async ({ page }) => {
    await api('/settings', {
      method: 'PUT',
      // Two per batch, so three films take two batches and the user still confirms once.
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
        conditions: [{ type: 'title_contains', value: ['akira', 'totoro', 'perfect blue'] }],
        exclusions: [],
      } satisfies Partial<RuleDraft>),
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
});
