import { describe, it, expect, vi, afterEach } from 'vitest';
import { fireEvent, screen, waitFor, within } from '@testing-library/svelte';
import userEvent from '@testing-library/user-event';

import { renderWithI18n } from '../test/render';
import { category, instance as fixtureInstance } from '../test/fixtures';
import { statusRevision } from '../lib/status.svelte';
import { api, ApiError } from '../api/client';
import type { Category, FolderSuggestion, MappingConflict, RootFolder } from '../api/types';
import { answerConfirmation } from '../test/confirm';
import { confirmation } from '../lib/confirm.svelte';
import { unloading } from '../test/leaving';
import { navigate, router } from '../lib/router.svelte';
import Categories from './Categories.svelte';
import { publishAccess } from '../lib/access.svelte';

function folder(over: Partial<RootFolder> = {}): RootFolder {
  return {
    id: 'rf1',
    instance_id: 'i1',
    arr_id: 1,
    path: '/data/films',
    free_space: 1_000_000_000,
    accessible: true,
    last_accessible_at: '2026-09-08 10:00:00',
    origin: 'arr',
    category: 'standard',
    last_synced_at: '2026-08-27 10:00:00',
    instance_name: 'Radarr',
    instance_type: 'radarr',
    ...over,
  };
}

const instance = (id: string, name: string) =>
  fixtureInstance({ id, name, last_sync_at: '2026-08-27 10:00:00' });

function show(
  folders: RootFolder[],
  categories: Category[],
  conflicts: MappingConflict[] = [],
  // One instance is the usual install, and a second makes the form's choice observable.
  instances = [instance('i1', 'Radarr')],
  suggestions: FolderSuggestion[] = [],
) {
  vi.spyOn(api, 'getFolderSuggestions').mockResolvedValue(suggestions);
  vi.spyOn(api, 'getRootFolders').mockResolvedValue(folders);
  vi.spyOn(api, 'getCategories').mockResolvedValue(categories);
  vi.spyOn(api, 'getMappingConflicts').mockResolvedValue(conflicts);
  vi.spyOn(api, 'getInstances').mockResolvedValue(instances);
  return renderWithI18n(Categories);
}

/** The destination form at the foot of `name`'s folders, opened. */
async function openDeclare(name = 'Radarr'): Promise<HTMLElement> {
  const summary = await screen.findByText(`Add a destination folder on ${name}`);
  await userEvent.click(summary);
  return summary.closest('details') as HTMLElement;
}

afterEach(() => vi.restoreAllMocks());

describe('Categories', () => {
  /**
   * The badge and the delete guard both read the `default_category` setting: a column beside it
   * would drift, and the guard would leave the real fallback deletable.
   */
  it('marks the default category and refuses to offer its deletion', async () => {
    show(
      [],
      [category({ name: 'standard', is_default: true }), category({ id: 'c2', name: 'anime' })],
    );

    await screen.findByText('standard');
    expect(screen.getByText('Default')).toBeTruthy();
    expect(screen.queryByRole('button', { name: 'Delete – standard' })).toBeNull();
    expect(screen.getByRole('button', { name: 'Delete – anime' })).toBeTruthy();
  });

  /** The server renames the row, the `%_category` columns and the setting in one transaction. */
  it('renames through the API and re-reads what the rename touched', async () => {
    const rename = vi.spyOn(api, 'renameCategory').mockResolvedValue(undefined as never);
    const reread = vi.spyOn(api, 'getCategories');
    show([], [category({ name: 'anime' })]);

    await fireEvent.click(await screen.findByRole('button', { name: 'Rename category – anime' }));
    const field = await screen.findByLabelText('Name');
    await fireEvent.input(field, { target: { value: 'animation' } });
    await fireEvent.submit(field.closest('form') as HTMLFormElement);

    await waitFor(() => expect(rename).toHaveBeenCalledWith('c1', 'animation'));
    await waitFor(() => expect(reread.mock.calls.length).toBeGreaterThan(1));
  });

  it('asks before a folder joins a category another folder holds, then says they share it', async () => {
    const update = vi.spyOn(api, 'updateRootFolderCategory').mockResolvedValue(undefined as never);
    show(
      [
        folder({ id: 'rf1', path: '/data/anime', category: 'anime' }),
        folder({ id: 'rf2', path: '/data/anime2', category: null }),
      ],
      [category({ name: 'anime' })],
    );
    const picker = await screen.findByLabelText('Category for /data/anime2');

    await userEvent.selectOptions(picker, 'anime');
    await fireEvent.click(screen.getByRole('button', { name: 'Save mappings' }));
    expect(await answerConfirmation(null)).toContain('/data/anime');
    expect(update).not.toHaveBeenCalled();
    expect(picker).toHaveValue('anime');

    await fireEvent.click(screen.getByRole('button', { name: 'Save mappings' }));
    await answerConfirmation('confirm');
    await waitFor(() => expect(update).toHaveBeenCalledWith('rf2', 'anime', true));

    show(
      [
        folder({ id: 'rf1', path: '/data/anime', category: 'anime' }),
        folder({ id: 'rf2', path: '/data/anime2', category: 'anime' }),
      ],
      [category({ name: 'anime' })],
    );
    expect(await screen.findByText('Shared with /data/anime2')).toBeTruthy();
    expect(screen.getByText('Shared with /data/anime')).toBeTruthy();
  });

  it('saves every pick together from a bar that counts them, each row marked', async () => {
    const update = vi.spyOn(api, 'updateRootFolderCategory').mockResolvedValue(undefined as never);
    show(
      [
        folder({ id: 'rf1', path: '/data/anime', category: null }),
        folder({ id: 'rf2', path: '/data/kids', category: null }),
      ],
      [category({ name: 'anime' }), category({ id: 'c2', name: 'kids' })],
    );

    await userEvent.selectOptions(
      await screen.findByLabelText('Category for /data/anime'),
      'anime',
    );
    await userEvent.selectOptions(screen.getByLabelText('Category for /data/kids'), 'kids');
    expect(update).not.toHaveBeenCalled();
    expect(screen.getByText('Folder mappings changed: 2')).toBeTruthy();
    expect(
      within(screen.getByRole('row', { name: /\/data\/kids/ })).getByText('Changed'),
    ).toBeTruthy();
    expect(screen.queryByRole('button', { name: /^Save – / })).toBeNull();

    await fireEvent.click(screen.getByRole('button', { name: 'Save mappings' }));

    await waitFor(() =>
      expect(update.mock.calls).toEqual([
        ['rf1', 'anime', false],
        ['rf2', 'kids', false],
      ]),
    );
    expect(await screen.findByText('Folder mappings saved: 2')).toBeTruthy();
    await waitFor(() => expect(screen.queryByText(/Folder mappings changed/)).toBeNull());
  });

  it('puts every pick back from the bar, writing nothing', async () => {
    const update = vi.spyOn(api, 'updateRootFolderCategory');
    show(
      [folder({ path: '/data/anime', category: 'standard' })],
      [category(), category({ id: 'c2', name: 'anime' })],
    );
    const picker = await screen.findByLabelText('Category for /data/anime');
    await userEvent.selectOptions(picker, 'anime');

    await fireEvent.click(screen.getByRole('button', { name: 'Discard' }));

    expect(picker).toHaveValue('standard');
    expect(screen.queryByText(/Folder mappings changed/)).toBeNull();
    expect(update).not.toHaveBeenCalled();
  });

  it('asks before the screen changes under a category not saved, and stays on Cancel', async () => {
    show([folder({ path: '/data/anime', category: null })], [category({ name: 'anime' })]);
    const picker = await screen.findByLabelText('Category for /data/anime');
    expect(unloading()).toBe(false);

    await userEvent.selectOptions(picker, 'anime');

    expect(unloading()).toBe(true);
    navigate('/rules');
    expect(await answerConfirmation(null)).toBe(
      'Leave without saving the categories picked for the folders? Unsaved changes: 1',
    );
    expect(router.path).not.toBe('/rules');
    expect(picker).toHaveValue('anime');
  });

  it('tells the shell its counters are out of date', async () => {
    vi.spyOn(api, 'updateRootFolderCategory').mockResolvedValue(undefined as never);
    show([folder({ path: '/data/anime', category: null })], [category({ name: 'anime' })]);

    const before = statusRevision();
    await userEvent.selectOptions(
      await screen.findByLabelText('Category for /data/anime'),
      'anime',
    );
    await fireEvent.click(screen.getByRole('button', { name: 'Save mappings' }));

    await waitFor(() => expect(statusRevision()).toBeGreaterThan(before));
  });

  it('says when a folder last answered rather than calling it broken', async () => {
    show([folder({ accessible: false, last_accessible_at: new Date().toISOString() })], []);

    const badge = await screen.findByText('Inaccessible');
    expect(badge.className).toContain('badge-warning');
    expect(badge.className).not.toContain('badge-danger');
    expect(screen.getByText(/Last answered/)).toBeInTheDocument();
  });

  it('says so plainly when it has never answered at all', async () => {
    show([folder({ accessible: false, last_accessible_at: null })], []);

    expect(await screen.findByText('Has never answered')).toBeInTheDocument();
  });

  it('declares a destination the instance does not report', async () => {
    const declare = vi
      .spyOn(api, 'declareRootFolder')
      .mockResolvedValue({ id: 'rf-x', path: '/media/movies/anime', verified: true });
    show([folder()], []);

    const path = within(await openDeclare()).getByLabelText('Destination folder');
    expect(path).toHaveAccessibleDescription(/checked against that instance/);
    await userEvent.type(path, '/media/movies/anime');
    await fireEvent.click(screen.getByRole('button', { name: 'Add' }));

    await waitFor(() => expect(declare).toHaveBeenCalledWith('i1', '/media/movies/anime'));
  });

  it('hands the focus to the path field once a destination is added', async () => {
    vi.spyOn(api, 'declareRootFolder').mockResolvedValue({
      id: 'rf-x',
      path: '/media/movies/anime',
      verified: true,
    });
    show([folder()], []);

    const path = within(await openDeclare()).getByLabelText('Destination folder');
    await userEvent.type(path, '/media/movies/anime');
    await userEvent.click(screen.getByRole('button', { name: 'Add' }));

    await waitFor(() => expect(document.activeElement).toBe(path));
  });

  it('declares a destination once however often the form is submitted', async () => {
    const declare = vi.spyOn(api, 'declareRootFolder').mockReturnValue(new Promise(() => {}));
    show([folder()], []);

    await userEvent.type(
      within(await openDeclare()).getByLabelText('Destination folder'),
      '/media/movies/anime',
    );
    const add = screen.getByRole('button', { name: 'Add' });
    await fireEvent.click(add);
    await fireEvent.submit(add.closest('form') as HTMLFormElement);

    expect(declare).toHaveBeenCalledTimes(1);
  });

  it('declares on the instance whose folders the form is opened under', async () => {
    const declare = vi
      .spyOn(api, 'declareRootFolder')
      .mockResolvedValue({ id: 'rf-x', path: '/data/anime', verified: true });
    show([folder()], [], [], [instance('i1', 'Radarr'), instance('i2', 'Sonarr')]);

    const form = await openDeclare('Sonarr');
    await userEvent.type(within(form).getByLabelText('Destination folder'), '/data/anime');
    await fireEvent.click(within(form).getByRole('button', { name: 'Add' }));

    await waitFor(() => expect(declare).toHaveBeenCalledWith('i2', '/data/anime'));
  });

  it('offers to remove only what it declared', async () => {
    show(
      [
        folder({ id: 'rf-arr', origin: 'arr' }),
        folder({ id: 'rf-mine', origin: 'declared', path: '/data/films/anime' }),
      ],
      [],
    );

    await screen.findByText('/data/films/anime');
    expect(screen.getAllByRole('button', { name: /^Remove/ })).toHaveLength(1);
  });

  it('drops the previous success when the next attempt is refused', async () => {
    vi.spyOn(api, 'declareRootFolder')
      .mockResolvedValueOnce({ id: 'rf-x', path: '/ok', verified: true })
      .mockRejectedValueOnce(new Error('the instance cannot see it'));
    show([folder()], []);

    const field = within(await openDeclare()).getByLabelText('Destination folder');
    await userEvent.type(field, '/ok');
    await fireEvent.click(screen.getByRole('button', { name: 'Add' }));
    expect(await screen.findByText(/added/)).toBeInTheDocument();

    await userEvent.type(field, '/mnt/typo');
    await fireEvent.click(screen.getByRole('button', { name: 'Add' }));

    await waitFor(() => expect(screen.queryByText(/added/)).toBeNull());
    expect(await screen.findByText(/cannot see it/)).toBeInTheDocument();
  });

  it('keeps a refused pick pending and names it beside the picks saved', async () => {
    vi.spyOn(api, 'updateRootFolderCategory').mockImplementation(async (id) => {
      if (id === 'rf2') throw new ApiError('anime already maps /data/other', 409, 'conflict');
      return {};
    });
    show(
      [
        folder({ id: 'rf1', path: '/data/films', category: 'standard' }),
        folder({ id: 'rf2', path: '/data/anime', category: 'standard' }),
      ],
      [category(), category({ id: 'c2', name: 'anime' }), category({ id: 'c3', name: 'kids' })],
    );
    await userEvent.selectOptions(await screen.findByLabelText('Category for /data/films'), 'kids');
    await userEvent.selectOptions(screen.getByLabelText('Category for /data/anime'), 'anime');

    await fireEvent.click(screen.getByRole('button', { name: 'Save mappings' }));

    expect(await screen.findByText('Folder mappings saved: 1. These could not be:')).toBeTruthy();
    expect(screen.getByText('/data/anime: anime already maps /data/other')).toBeTruthy();
    await waitFor(() => expect(screen.getByText('Folder mappings changed: 1')).toBeTruthy());
    expect(screen.getByLabelText('Category for /data/anime')).toHaveValue('anime');
  });

  it('says why a single pick was refused, and keeps it', async () => {
    vi.spyOn(api, 'updateRootFolderCategory').mockRejectedValue(
      new ApiError('anime already maps /data/other', 409, 'conflict'),
    );
    show(
      [folder({ path: '/data/anime', category: 'standard' })],
      [category(), category({ id: 'c2', name: 'anime' })],
    );
    const select = (await screen.findByLabelText('Category for /data/anime')) as HTMLSelectElement;

    await userEvent.selectOptions(select, 'anime');
    await fireEvent.click(screen.getByRole('button', { name: 'Save mappings' }));

    expect(await screen.findByText('anime already maps /data/other')).toBeInTheDocument();
    expect(select.value).toBe('anime');
  });

  it('keeps a refused destination in its field', async () => {
    vi.spyOn(api, 'declareRootFolder').mockRejectedValue(new Error('the instance cannot see it'));
    show([folder()], []);
    const field = within(await openDeclare()).getByLabelText(
      'Destination folder',
    ) as HTMLInputElement;

    await userEvent.type(field, '/mnt/typo');
    await fireEvent.click(screen.getByRole('button', { name: 'Add' }));

    expect(await screen.findByText(/cannot see it/)).toBeInTheDocument();
    // A success clears the field once the folders are read again, so wait for that read.
    await waitFor(() => expect(api.getRootFolders).toHaveBeenCalledTimes(2));
    await new Promise((resolve) => setTimeout(resolve, 0));
    expect(field.value).toBe('/mnt/typo');
  });

  it('keeps a refused new category in its dialog and says why there', async () => {
    vi.spyOn(api, 'createCategory').mockRejectedValue(
      new ApiError('A category named anime exists', 409, 'conflict'),
    );
    show([folder()], [category()]);

    await fireEvent.click(await screen.findByRole('button', { name: 'New category' }));
    const dialog = await screen.findByRole('dialog');
    // What the field accepts is said to whoever types in it, not only to an eye on the line below.
    expect(within(dialog).getByLabelText('Name')).toHaveAccessibleDescription(/Letters, digits/);
    await userEvent.type(within(dialog).getByLabelText('Name'), 'anime');
    await fireEvent.click(within(dialog).getByRole('button', { name: 'Create' }));

    expect(await within(dialog).findByText('A category named anime exists')).toBeInTheDocument();
    expect((within(dialog).getByLabelText('Name') as HTMLInputElement).value).toBe('anime');
  });

  it('keeps a refused rename in its dialog and says why there', async () => {
    vi.spyOn(api, 'renameCategory').mockRejectedValue(
      new ApiError('A category named kids exists', 409, 'conflict'),
    );
    show([folder()], [category({ name: 'anime' })]);

    await fireEvent.click(await screen.findByRole('button', { name: /Rename category – anime/ }));
    const dialog = await screen.findByRole('dialog');
    const field = within(dialog).getByLabelText('Name') as HTMLInputElement;
    expect(field).toHaveAccessibleDescription(/all follow the new name/);
    await userEvent.clear(field);
    await userEvent.type(field, 'kids');
    await fireEvent.click(within(dialog).getByRole('button', { name: 'Save' }));

    expect(await within(dialog).findByText('A category named kids exists')).toBeInTheDocument();
    expect((within(dialog).getByLabelText('Name') as HTMLInputElement).value).toBe('kids');
  });

  /** Every other deletion asks first, and these two remove as much. */
  it('asks before removing a destination, and removes nothing on Cancel', async () => {
    const remove = vi.spyOn(api, 'deleteRootFolder');
    show([folder({ path: '/data/anime', origin: 'declared' })], [category()]);

    await fireEvent.click(await screen.findByRole('button', { name: 'Remove – /data/anime' }));

    expect(await answerConfirmation(null)).toBe(
      'Remove the destination /data/anime? A category mapped to it no longer routes there. Nothing is deleted on disk.',
    );
    expect(remove).not.toHaveBeenCalled();
  });

  it('asks before deleting a category, and deletes nothing on Cancel', async () => {
    const remove = vi.spyOn(api, 'deleteCategory');
    const kids = category({ id: 'c2', name: 'kids', is_default: false, root_folder_count: 0 });
    show([folder()], [kids]);

    await fireEvent.click(await screen.findByRole('button', { name: 'Delete – kids' }));

    expect(await answerConfirmation(null)).toBe('Delete the category "kids"?');
    expect(remove).not.toHaveBeenCalled();
  });

  /** The table counts rules and folders: what else holds a category is said before the refusal. */
  it('names what still holds a category and offers no removal the server would refuse', async () => {
    const remove = vi.spyOn(api, 'deleteCategory');
    const held = category({
      id: 'c2',
      name: 'kids',
      root_folder_count: 0,
      exception_count: 1,
      rule_test_count: 2,
    });
    show([folder()], [held]);

    await fireEvent.click(await screen.findByRole('button', { name: 'Delete – kids' }));

    await waitFor(() => expect(confirmation.request?.choices).toEqual([]));
    expect(await answerConfirmation(null)).toBe(
      'The category "kids" is still in use, so it cannot be deleted. Rules: 0, root folders: 0, exceptions: 1, rule tests: 2.',
    );
    expect(remove).not.toHaveBeenCalled();
  });

  it('shows a refusal without waiting for the page to reload', async () => {
    vi.spyOn(api, 'declareRootFolder').mockRejectedValue(new Error('the instance cannot see it'));
    show([folder()], []);
    const field = within(await openDeclare()).getByLabelText('Destination folder');
    vi.spyOn(api, 'getRootFolders').mockReturnValue(new Promise(() => {}));

    await userEvent.type(field, '/mnt/typo');
    await fireEvent.click(screen.getByRole('button', { name: 'Add' }));

    expect(await screen.findByText(/cannot see it/)).toBeInTheDocument();
  });

  /**
   * An unmapped category makes a decision `skip`, never an error, so the screen offers "mapped to
   * nothing" as a deliberate choice.
   */
  it('sends null, not an empty string, when a folder is unmapped', async () => {
    const update = vi.spyOn(api, 'updateRootFolderCategory').mockResolvedValue(undefined as never);
    show([folder({ path: '/data/films' })], [category()]);

    await userEvent.selectOptions(await screen.findByLabelText('Category for /data/films'), '');
    await fireEvent.click(screen.getByRole('button', { name: 'Save mappings' }));

    await waitFor(() => expect(update).toHaveBeenCalledWith('rf1', null, false));
  });

  it('lists the conflicts of one severity in one banner', async () => {
    const conflict = (severity: 'error' | 'warning', message: string) => ({
      kind: 'unmapped',
      severity,
      instance_name: 'Radarr',
      category: 'anime',
      message,
    });
    const { container } = show(
      [folder()],
      [category()],
      [
        conflict('warning', 'Nothing maps "kids"'),
        conflict('error', 'Two folders claim "anime"'),
        conflict('warning', 'Nothing maps "docs"'),
      ],
    );

    await screen.findByText('Nothing maps "kids"');
    const warnings = container.querySelectorAll('.banner-warning');
    expect(warnings).toHaveLength(1);
    expect(warnings[0]!.querySelectorAll('li')).toHaveLength(2);
    const errors = container.querySelectorAll('.banner-danger li');
    expect(errors).toHaveLength(1);
    expect(errors[0]?.textContent).toContain('Two folders claim "anime"');
  });

  /** The screen maps folders per instance, so a warning lands on a group it can be read in. */
  it('groups the folders under their instance, and an instance with none under its sync', async () => {
    show(
      [folder({ path: '/data/films' })],
      [category()],
      [],
      [instance('i1', 'Radarr'), fixtureInstance({ id: 'i2', name: 'Radarr 4K' })],
    );

    const films = (await screen.findByText('/data/films')).closest('tbody') as HTMLElement;
    expect(within(films).getAllByRole('row')[0]).toHaveTextContent(/^Radarr\s*Radarr \(Movies\)/);
    expect(within(films).queryByRole('button', { name: /^Sync now/ })).toBeNull();
    const unsynced = screen.getByText('Radarr 4K').closest('tbody') as HTMLElement;
    expect(within(unsynced).getByText('Never synced')).toBeTruthy();
    expect(
      within(unsynced).getByText(
        'No root folder known yet: sync this instance to list its folders.',
      ),
    ).toBeTruthy();
    expect(within(unsynced).getByRole('button', { name: 'Sync now – Radarr 4K' })).toBeTruthy();
    const headers = screen.getAllByRole('columnheader').map((header) => header.textContent);
    expect(headers).not.toContain('Instance');
    expect(headers).not.toContain('Type');
  });

  it('syncs an instance from its group, then reads the folders again', async () => {
    const sync = vi.spyOn(api, 'syncInstance').mockResolvedValue({
      instance_id: 'i2',
      instance_name: 'Radarr 4K',
      media: 4,
      root_folders: 2,
      removed: 0,
      held: 0,
      error: null,
    } as never);
    show([], [category()], [], [fixtureInstance({ id: 'i2', name: 'Radarr 4K' })]);

    await fireEvent.click(await screen.findByRole('button', { name: 'Sync now – Radarr 4K' }));

    await waitFor(() => expect(sync).toHaveBeenCalledWith('i2', expect.anything()));
    expect(await screen.findByText('Radarr 4K is synced. Titles: 4, root folders: 2')).toBeTruthy();
    expect(api.getRootFolders).toHaveBeenCalledTimes(2);
  });

  it('reads its folders again every minute, keeping the picks not saved', async () => {
    vi.useFakeTimers({ shouldAdvanceTime: true });
    try {
      show([folder({ path: '/data/anime', category: null })], [category({ name: 'anime' })]);
      const picker = await screen.findByLabelText('Category for /data/anime');
      await userEvent.selectOptions(picker, 'anime');

      await vi.advanceTimersByTimeAsync(60_000);

      await waitFor(() => expect(api.getRootFolders).toHaveBeenCalledTimes(2));
      expect(screen.getByLabelText('Category for /data/anime')).toHaveValue('anime');
    } finally {
      vi.useRealTimers();
    }
  });

  it('shows only its empty state while no instance is set up', async () => {
    show([], [category()], [], []);

    expect(
      await screen.findByText('No root folder discovered yet. Add an instance and sync it.'),
    ).toBeTruthy();
    expect(screen.queryByText(/^Add a destination folder on/)).toBeNull();
  });

  /** "Radarr 4K: No root folder on 'Radarr 4K'..." says the name twice. */
  it('leaves out the lead of a warning whose sentence names its instance', async () => {
    const warn = (instance_name: string, message: string): MappingConflict => ({
      kind: 'unmapped_category',
      severity: 'warning',
      instance_name,
      category: 'anime',
      message,
    });
    const { container } = show(
      [folder()],
      [category()],
      [
        warn('Radarr 4K', "No root folder on 'Radarr 4K' is mapped to 'anime'"),
        warn('Radarr', 'A folder is mapped to a category that no longer exists'),
      ],
    );

    await screen.findByText(/No root folder on/);
    expect(
      [...container.querySelectorAll('.banner-warning li')].map((item) => item.textContent?.trim()),
    ).toEqual([
      "No root folder on 'Radarr 4K' is mapped to 'anime'",
      'Radarr: A folder is mapped to a category that no longer exists',
    ]);
  });

  it('opens each category dialog on its name field', async () => {
    show([], [category({ name: 'anime' })]);

    await fireEvent.click(await screen.findByRole('button', { name: 'Rename category – anime' }));
    expect(document.activeElement).toBe(await screen.findByLabelText('Name'));
    await fireEvent.click(screen.getByRole('button', { name: 'Cancel' }));

    await fireEvent.click(await screen.findByRole('button', { name: 'New category' }));
    expect(document.activeElement).toBe(await screen.findByLabelText('Name'));
  });
});

describe('the focus after a removal', () => {
  it('goes to the destination that took the place of the removed one', async () => {
    const next = folder({ id: 'rf2', path: '/data/anime', origin: 'declared' });
    show([folder({ origin: 'declared' }), next], [category()]);
    vi.spyOn(api, 'deleteRootFolder').mockResolvedValue(undefined as never);

    await fireEvent.click(await screen.findByRole('button', { name: 'Remove – /data/films' }));
    vi.spyOn(api, 'getRootFolders').mockResolvedValue([next]);
    await answerConfirmation();

    await waitFor(() =>
      expect(document.activeElement).toBe(
        screen.getByRole('button', { name: 'Remove – /data/anime' }),
      ),
    );
  });

  it('goes to the category that took the place of the deleted one', async () => {
    const next = category({ id: 'c2', name: 'anime' });
    show([], [category({ root_folder_count: 0 }), next]);
    vi.spyOn(api, 'deleteCategory').mockResolvedValue(undefined as never);

    await fireEvent.click(await screen.findByRole('button', { name: 'Delete – standard' }));
    vi.spyOn(api, 'getCategories').mockResolvedValue([next]);
    await answerConfirmation();

    await waitFor(() =>
      expect(document.activeElement).toBe(screen.getByRole('button', { name: 'Delete – anime' })),
    );
  });
});

/** A category, a mapping and a destination are written: a reader reads where each folder goes. */
describe('for a reader', () => {
  const FOLDERS = () => [
    folder(),
    folder({ id: 'rf2', path: '/data/new', category: null }),
    folder({ id: 'rf3', path: '/data/kept', origin: 'declared', category: 'anime' }),
  ];
  const CATEGORIES = () => [
    category({ id: 'c1', name: 'standard', is_default: true }),
    category({ id: 'c2', name: 'anime' }),
  ];
  const offered = () =>
    ['New category', 'Remove – /data/kept', 'Rename category – anime', 'Delete – anime'].filter(
      (name) => screen.queryByRole('button', { name }),
    );

  it('offers the owner every write, and a reader the mappings to read alone', async () => {
    const { unmount } = show(FOLDERS(), CATEGORIES());
    await screen.findByRole('button', { name: 'Delete – anime' });
    expect(offered()).toHaveLength(4);
    expect(screen.getByLabelText('Category for /data/films')).toBeInTheDocument();
    expect(screen.getByText('Add a destination folder on Radarr')).toBeInTheDocument();
    unmount();

    publishAccess(true);
    show(FOLDERS(), CATEGORIES());

    const mapped = await screen.findByRole('row', { name: /\/data\/films/ });
    expect(within(mapped).getByText('standard')).toBeInTheDocument();
    expect(
      within(screen.getByRole('row', { name: /\/data\/new/ })).getByText('(unmapped)'),
    ).toBeTruthy();
    expect(offered()).toEqual([]);
    expect(screen.queryByRole('combobox')).toBeNull();
    expect(screen.queryByText(/^Add a destination folder on/)).toBeNull();
  });
});

function suggestion(over: Partial<FolderSuggestion> = {}): FolderSuggestion {
  return {
    root_folder_id: 'rf-anime',
    instance_id: 'i1',
    instance_name: 'Radarr',
    path: '/movies/anime',
    category: 'anime',
    choices: ['anime'],
    reason: 'The folder name says anime.',
    template: { id: 'anime', name: 'Anime' },
    dismissed: false,
    ...over,
  };
}

const unmapped = (id: string, path: string) => folder({ id, path, category: null });

describe('folder suggestions', () => {
  it('maps a suggested folder, making its category first, and offers the rules after', async () => {
    const create = vi
      .spyOn(api, 'createCategory')
      .mockResolvedValue(category({ id: 'c-anime', name: 'anime' }));
    const map = vi.spyOn(api, 'updateRootFolderCategory').mockResolvedValue({});
    show([unmapped('rf-anime', '/movies/anime')], [category({ name: 'standard' })], [], undefined, [
      suggestion(),
    ]);

    const card = await screen.findByRole('region', { name: 'Suggested from folder names' });
    expect(within(card).getByText('The folder name says anime.')).toBeTruthy();
    await fireEvent.click(within(card).getByRole('button', { name: 'Map – /movies/anime' }));

    await screen.findByText('/movies/anime is mapped to anime.');
    expect(create).toHaveBeenCalledWith({ name: 'anime' });
    expect(map).toHaveBeenCalledWith('rf-anime', 'anime');
    const rules = within(card).getByRole('link', { name: 'Add the Anime rules – /movies/anime' });
    expect(rules.getAttribute('href')).toBe('/rules?template=anime&category=anime');
    expect(within(card).getByRole('button', { name: 'Undo – /movies/anime' })).toBeTruthy();
  });

  it('undoes a map, removing the category it made and nothing else', async () => {
    vi.spyOn(api, 'createCategory').mockResolvedValue(category({ id: 'c-anime', name: 'anime' }));
    const map = vi.spyOn(api, 'updateRootFolderCategory').mockResolvedValue({});
    const remove = vi.spyOn(api, 'deleteCategory').mockResolvedValue({});
    const kids = suggestion({
      root_folder_id: 'rf-kids',
      path: '/movies/kids',
      category: 'kids',
      choices: ['kids'],
      template: null,
    });
    show(
      [unmapped('rf-anime', '/movies/anime'), unmapped('rf-kids', '/movies/kids')],
      [category({ name: 'standard' }), category({ id: 'c-kids', name: 'kids' })],
      [],
      undefined,
      [suggestion(), kids],
    );
    const card = await screen.findByRole('region', { name: 'Suggested from folder names' });

    await fireEvent.click(within(card).getByRole('button', { name: 'Map – /movies/kids' }));
    await fireEvent.click(await within(card).findByRole('button', { name: 'Undo – /movies/kids' }));
    await screen.findByText('/movies/kids leads to no category again.');
    expect(map).toHaveBeenLastCalledWith('rf-kids', null);
    expect(remove).not.toHaveBeenCalled();

    await fireEvent.click(within(card).getByRole('button', { name: 'Map – /movies/anime' }));
    await fireEvent.click(
      await within(card).findByRole('button', { name: 'Undo – /movies/anime' }),
    );
    await waitFor(() => expect(remove).toHaveBeenCalledWith('c-anime'));
  });

  /** One new category for two folders: whichever row is undone first leaves it to the other. */
  it('keeps a category another mapped folder still uses, and removes it with the last undo', async () => {
    vi.spyOn(api, 'createCategory')
      .mockResolvedValueOnce(category({ id: 'c-anime', name: 'anime' }))
      .mockRejectedValueOnce(new ApiError('Taken.', 409, 'conflict', null, null, [], 'name_taken'));
    vi.spyOn(api, 'updateRootFolderCategory').mockResolvedValue({});
    const inUse = new ApiError('In use.', 409, 'conflict', null, null, [], 'in_use');
    const remove = vi
      .spyOn(api, 'deleteCategory')
      .mockRejectedValueOnce(inUse)
      .mockResolvedValue({});
    const series = suggestion({
      root_folder_id: 'rf-tv-anime',
      instance_id: 'i2',
      instance_name: 'Sonarr',
      path: '/tv/anime',
    });
    show([], [category({ name: 'standard' })], [], undefined, [suggestion(), series]);
    const card = await screen.findByRole('region', { name: 'Suggested from folder names' });
    await fireEvent.click(within(card).getByRole('button', { name: 'Map all' }));
    await screen.findByText('Folders mapped: 2.');

    await fireEvent.click(within(card).getByRole('button', { name: 'Undo – /movies/anime' }));

    expect(await screen.findByText('/movies/anime leads to no category again.')).toBeTruthy();
    expect(within(card).queryByRole('button', { name: 'Undo – /movies/anime' })).toBeNull();
    await fireEvent.click(within(card).getByRole('button', { name: 'Undo – /tv/anime' }));
    await screen.findByText('/tv/anime leads to no category again.');
    expect(remove.mock.calls).toEqual([['c-anime'], ['c-anime']]);
  });

  it('maps every row that names one category, and leaves a choice to the person', async () => {
    vi.spyOn(api, 'createCategory').mockResolvedValue(category({ name: 'x' }));
    const map = vi.spyOn(api, 'updateRootFolderCategory').mockResolvedValue({});
    const ambiguous = suggestion({
      root_folder_id: 'rf-both',
      path: '/movies/kids-anime',
      category: null,
      choices: ['kids', 'anime'],
      reason: 'The folder name says several categories: pick the one it holds.',
      template: null,
    });
    const docs = suggestion({
      root_folder_id: 'rf-docs',
      path: '/movies/docs',
      category: 'documentaries',
      choices: ['documentaries'],
    });
    show([], [category({ name: 'standard' })], [], undefined, [suggestion(), ambiguous, docs]);
    const card = await screen.findByRole('region', { name: 'Suggested from folder names' });

    expect(within(card).getByRole('button', { name: 'Map – /movies/kids-anime' })).toHaveProperty(
      'disabled',
      true,
    );
    await fireEvent.click(within(card).getByRole('button', { name: 'Map all' }));

    await screen.findByText('Folders mapped: 2.');
    expect(map.mock.calls.map(([id]) => id)).toEqual(['rf-anime', 'rf-docs']);
  });

  it('says which folders a Map all could not map, and maps the others', async () => {
    vi.spyOn(api, 'createCategory').mockResolvedValue(category({ name: 'x' }));
    vi.spyOn(api, 'updateRootFolderCategory').mockImplementation(async (id) => {
      if (id === 'rf-docs') throw new ApiError('Taken meanwhile.', 409, 'conflict');
      return {};
    });
    const docs = suggestion({
      root_folder_id: 'rf-docs',
      path: '/movies/docs',
      category: 'documentaries',
      choices: ['documentaries'],
    });
    show([], [category({ name: 'standard' })], [], undefined, [suggestion(), docs]);
    const card = await screen.findByRole('region', { name: 'Suggested from folder names' });

    await fireEvent.click(within(card).getByRole('button', { name: 'Map all' }));

    await screen.findByText('Folders mapped: 1. These could not be:');
    expect(screen.getByText('/movies/docs: Taken meanwhile.')).toBeTruthy();
  });

  /** A pressed button that turns disabled, then goes with its row, leaves the focus on the page. */
  describe('hands the focus on', () => {
    const docs = suggestion({
      root_folder_id: 'rf-docs',
      path: '/movies/docs',
      category: 'documentaries',
      choices: ['documentaries'],
    });

    async function press(name: string) {
      const card = await screen.findByRole('region', { name: 'Suggested from folder names' });
      const button = within(card).getByRole('button', { name });
      button.focus();
      await fireEvent.click(button);
      return card;
    }

    it.each([
      ['Map', 'Map – /movies/docs'],
      ['Dismiss', 'Dismiss – /movies/docs'],
    ])('to the row in its place once %s took the row away', async (action, next) => {
      vi.spyOn(api, 'createCategory').mockResolvedValue(category({ name: 'anime' }));
      vi.spyOn(api, 'updateRootFolderCategory').mockResolvedValue({});
      vi.spyOn(api, 'dismissFolderSuggestion').mockResolvedValue({ dismissed: true });
      show([], [category({ name: 'standard' })], [], undefined, [suggestion(), docs]);
      await screen.findByRole('region', { name: 'Suggested from folder names' });
      // A mapped folder suggests nothing, a dismissed one is listed apart.
      const after = action === 'Map' ? [docs] : [suggestion({ dismissed: true }), docs];
      vi.mocked(api.getFolderSuggestions).mockResolvedValue(after);

      await press(`${action} – /movies/anime`);

      await waitFor(() => expect(document.activeElement).toHaveAccessibleName(next));
    });

    it('to the same row once it is suggested again', async () => {
      vi.spyOn(api, 'restoreFolderSuggestion').mockResolvedValue({ deleted: true });
      show([], [category({ name: 'standard' })], [], undefined, [suggestion({ dismissed: true })]);
      await fireEvent.click(
        await screen.findByRole('button', { name: 'Show the dismissed suggestions (1)' }),
      );
      vi.mocked(api.getFolderSuggestions).mockResolvedValue([suggestion()]);

      await press('Suggest again – /movies/anime');

      await waitFor(() =>
        expect(document.activeElement).toHaveAccessibleName('Dismiss – /movies/anime'),
      );
    });

    it.each(['Map all', 'Undo – /movies/anime'])(
      'to the card title once %s leaves nothing in its place',
      async (name) => {
        vi.spyOn(api, 'createCategory').mockResolvedValue(
          category({ id: 'c-anime', name: 'anime' }),
        );
        vi.spyOn(api, 'updateRootFolderCategory').mockResolvedValue({});
        vi.spyOn(api, 'deleteCategory').mockResolvedValue({});
        show([], [category({ name: 'standard' })], [], undefined, [suggestion(), docs]);
        if (name.startsWith('Undo')) await press('Map – /movies/anime');
        await screen.findByRole('button', { name: /^Map all|^Undo/ });
        vi.mocked(api.getFolderSuggestions).mockResolvedValue([]);

        await press(name);

        await waitFor(() =>
          expect(document.activeElement).toBe(
            screen.getByRole('heading', { name: 'Suggested from folder names' }),
          ),
        );
      },
    );
  });

  it('hides a dismissed suggestion and shows it again on demand', async () => {
    const dismiss = vi.spyOn(api, 'dismissFolderSuggestion').mockResolvedValue({ dismissed: true });
    const restore = vi.spyOn(api, 'restoreFolderSuggestion').mockResolvedValue({ deleted: true });
    show([], [category({ name: 'standard' })], [], undefined, [suggestion()]);
    const card = await screen.findByRole('region', { name: 'Suggested from folder names' });

    vi.spyOn(api, 'getFolderSuggestions').mockResolvedValue([suggestion({ dismissed: true })]);
    await fireEvent.click(within(card).getByRole('button', { name: 'Dismiss – /movies/anime' }));
    expect(dismiss).toHaveBeenCalledWith('rf-anime');
    await waitFor(() => expect(within(card).queryByText('/movies/anime')).toBeNull());

    await fireEvent.click(
      within(card).getByRole('button', { name: 'Show the dismissed suggestions (1)' }),
    );
    await fireEvent.click(
      within(card).getByRole('button', { name: 'Suggest again – /movies/anime' }),
    );
    expect(restore).toHaveBeenCalledWith('rf-anime');
  });

  /**
   * The shell publishes the access once `/auth/me` answers, while the screen already loads: a
   * load restarted then would cancel every request of the first.
   */
  it('loads once when the shell learns who reads', async () => {
    publishAccess(null);
    show([folder()], [category({ name: 'standard' })]);
    await screen.findByText('/data/films');

    publishAccess(false);
    await new Promise((resolve) => setTimeout(resolve, 0));

    expect(api.getRootFolders).toHaveBeenCalledTimes(1);
  });

  it('shows no card while no folder name suggests anything', async () => {
    show([folder()], [category({ name: 'standard' })]);
    await screen.findByText('/data/films');
    expect(screen.queryByRole('region', { name: 'Suggested from folder names' })).toBeNull();
  });
});
