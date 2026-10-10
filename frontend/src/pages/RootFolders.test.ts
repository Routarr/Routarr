import { describe, it, expect, vi, afterEach } from 'vitest';
import { fireEvent, screen, waitFor, within } from '@testing-library/svelte';
import userEvent from '@testing-library/user-event';

import { renderWithI18n } from '../test/render';
import { instance as fixtureInstance } from '../test/fixtures';
import { statusRevision } from '../lib/status.svelte';
import { api, ApiError } from '../api/client';
import type { Category, MappingConflict, RootFolder } from '../api/types';
import { answerConfirmation } from '../test/confirm';
import { unloading } from '../test/leaving';
import { navigate, router } from '../lib/router.svelte';
import RootFolders from './RootFolders.svelte';

/**
 * Categories are free-form strings joined by name, with no foreign key, so this
 * screen is where the names in four tables are kept in step. Deleting one that
 * is in use is refused by the API precisely because the database would not stop
 * it, and the *default* category cannot be deleted at all, since an unmatched
 * item has to land somewhere.
 */

const STRINGS = {
  DeclareDestination: 'Destination folder',
  DeclareDestinationPlaceholder: '/media/movies/anime',
  DeclareDestinationHelp: 'A folder the instance can reach.',
  AddDestination: 'Add',
  DestinationDeclared: "Destination '{path}' added",
  DestinationRemoved: "Destination '{path}' removed",
  Remove: 'Remove',
  Instance: 'Instance',
  RootFolders: 'Categories and folders',
  Categories: 'Categories',
  FolderMappings: 'Folder mappings',
  NewCategory: 'New category',
  RenameCategory: 'Rename category',
  CategoryRenamed: 'Category renamed',
  DefaultCategoryBadge: 'default',
  Delete: 'Delete',
  Unmapped: 'Unmapped',
  CategoryForFolder: 'Category for {path}',
  MappingUpdated: 'Mapping updated',
  Name: 'Name',
  Save: 'Save',
  None: '-',
  Accessible: 'accessible',
  Inaccessible: 'unreachable',
  LastAnswered: 'Last answered {since}',
  NeverAnswered: 'Has never answered',
  NoRootFolderDiscovered: 'No root folder',
  Create: 'Create',
  Cancel: 'Cancel',
  CategoryCreated: 'Category {name} created',
  CategoryDeleted: 'Category deleted',
  ConfirmRemoveDestination: 'Remove the destination {path}?',
  ConfirmDeleteCategory: 'Delete the category "{name}"?',
  ConfirmLeaveUnsavedMappings: 'Leave without saving the folder categories? Unsaved: {count}',
};

function category(over: Partial<Category> = {}): Category {
  return {
    id: 'c1',
    name: 'standard',
    description: null,
    is_default: false,
    display_order: 1,
    created_at: '2026-08-27 10:00:00',
    rule_count: 0,
    root_folder_count: 1,
    ...over,
  };
}

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

const instance = (id: string, name: string) => fixtureInstance({ id, name });

function show(
  folders: RootFolder[],
  categories: Category[],
  conflicts: MappingConflict[] = [],
  // The declaration form needs somewhere to send a path. One instance is the
  // ordinary install, and a second is what makes the choice observable.
  instances = [instance('i1', 'Radarr')],
) {
  vi.spyOn(api, 'getRootFolders').mockResolvedValue(folders);
  vi.spyOn(api, 'getCategories').mockResolvedValue(categories);
  vi.spyOn(api, 'getMappingConflicts').mockResolvedValue(conflicts);
  vi.spyOn(api, 'getInstances').mockResolvedValue(instances);
  return renderWithI18n(RootFolders, { strings: STRINGS });
}

afterEach(() => vi.restoreAllMocks());

describe('Root folders', () => {
  /**
   * The badge and the delete guard both read the `default_category` setting.
   * An `is_default` column beside it would be kept in step by nothing: the
   * interface would badge one category while the engine fell back to another,
   * and the guard would protect the badged one, leaving the real fallback
   * deletable.
   */
  it('marks the default category and refuses to offer its deletion', async () => {
    show(
      [],
      [category({ name: 'standard', is_default: true }), category({ id: 'c2', name: 'anime' })],
    );

    await screen.findByText('standard');
    expect(screen.getByText('default')).toBeTruthy();
    expect(screen.queryByRole('button', { name: 'Delete – standard' })).toBeNull();
    // Every other category is deletable.
    expect(screen.getByRole('button', { name: 'Delete – anime' })).toBeTruthy();
  });

  /**
   * Nothing cascades: renaming updates the row, the four `%_category` columns
   * and the `default_category` setting, in one transaction on the server. The
   * screen's job is to send the new name and then re-read everything.
   *
   * `{@const}` is reactive, so clearing the dialog's state before reading the id
   * reads it as null and the rename never leaves the browser, which no compiler
   * sees.
   */
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

  /**
   * On Windows and Linux an arrow key on a closed select fires `change`, and
   * writing on `change` would send every category passed on the way to the
   * server. Choosing is free, Save writes.
   */
  it('writes the category chosen only when Save is pressed', async () => {
    const update = vi.spyOn(api, 'updateRootFolderCategory').mockResolvedValue(undefined as never);
    show([folder({ path: '/data/anime', category: null })], [category({ name: 'anime' })]);

    await userEvent.selectOptions(
      await screen.findByLabelText('Category for /data/anime'),
      'anime',
    );
    expect(update).not.toHaveBeenCalled();

    await fireEvent.click(screen.getByRole('button', { name: 'Save – /data/anime' }));
    await waitFor(() => expect(update).toHaveBeenCalledWith('rf1', 'anime'));
  });

  /** A category picked and not saved is dropped by leaving, so leaving asks first. */
  it('asks before the screen changes under a category not saved, and stays on Cancel', async () => {
    show([folder({ path: '/data/anime', category: null })], [category({ name: 'anime' })]);
    const picker = await screen.findByLabelText('Category for /data/anime');
    expect(unloading()).toBe(false);

    await userEvent.selectOptions(picker, 'anime');

    expect(unloading()).toBe(true);
    navigate('/rules');
    expect(await answerConfirmation(null)).toBe(
      'Leave without saving the folder categories? Unsaved: 1',
    );
    expect(router.path).not.toBe('/rules');
    expect(picker).toHaveValue('anime');
  });

  /**
   * Mapping a category is what clears the "categories mapped to nothing"
   * warning, and the shell counts that warning in two places. It owns the
   * request, and this screen's job is to say that the answer changed.
   */
  it('tells the shell its counters are out of date', async () => {
    vi.spyOn(api, 'updateRootFolderCategory').mockResolvedValue(undefined as never);
    show([folder({ path: '/data/anime', category: null })], [category({ name: 'anime' })]);

    const before = statusRevision();
    await userEvent.selectOptions(
      await screen.findByLabelText('Category for /data/anime'),
      'anime',
    );
    await fireEvent.click(screen.getByRole('button', { name: 'Save – /data/anime' }));

    await waitFor(() => expect(statusRevision()).toBeGreaterThan(before));
  });

  /**
   * A NAS that spins down is reported unreachable every night, so a folder that
   * did not answer is stated with *when* it last did rather than as a fault: an
   * error badge for a disk that is merely asleep is how a diagnostic gets
   * ignored. No threshold decides which is which: the reader knows their own
   * hardware, and needs the date to judge.
   */
  it('says when a folder last answered rather than calling it broken', async () => {
    show([folder({ accessible: false, last_accessible_at: new Date().toISOString() })], []);

    const badge = await screen.findByText('unreachable');
    expect(badge.className).toContain('badge-warning');
    expect(badge.className).not.toContain('badge-danger');
    expect(screen.getByText(/Last answered/)).toBeInTheDocument();
  });

  it('says so plainly when it has never answered at all', async () => {
    show([folder({ accessible: false, last_accessible_at: null })], []);

    expect(await screen.findByText('Has never answered')).toBeInTheDocument();
  });

  /**
   * A target need not be a root folder in Radarr or Sonarr already: routing
   * into `/media/movies/anime` would otherwise mean declaring it there first.
   * What an operator wants is one root folder per Arr and the targets beneath
   * it named here.
   */
  it('declares a destination the instance does not report', async () => {
    const declare = vi
      .spyOn(api, 'declareRootFolder')
      .mockResolvedValue({ id: 'rf-x', path: '/media/movies/anime', verified: true });
    show([folder()], []);

    await userEvent.type(await screen.findByLabelText('Destination folder'), '/media/movies/anime');
    await fireEvent.click(screen.getByRole('button', { name: 'Add' }));

    await waitFor(() => expect(declare).toHaveBeenCalledWith('i1', '/media/movies/anime'));
  });

  /** Add turns disabled with the path it cleared, so the focus goes to the next path. */
  it('hands the focus to the path field once a destination is added', async () => {
    vi.spyOn(api, 'declareRootFolder').mockResolvedValue({
      id: 'rf-x',
      path: '/media/movies/anime',
      verified: true,
    });
    show([folder()], []);

    const path = await screen.findByLabelText('Destination folder');
    await userEvent.type(path, '/media/movies/anime');
    await userEvent.click(screen.getByRole('button', { name: 'Add' }));

    await waitFor(() => expect(document.activeElement).toBe(path));
  });

  it('declares a destination once however often the form is submitted', async () => {
    const declare = vi.spyOn(api, 'declareRootFolder').mockReturnValue(new Promise(() => {}));
    show([folder()], []);

    await userEvent.type(await screen.findByLabelText('Destination folder'), '/media/movies/anime');
    const add = screen.getByRole('button', { name: 'Add' });
    await fireEvent.click(add);
    await fireEvent.submit(add.closest('form') as HTMLFormElement);

    expect(declare).toHaveBeenCalledTimes(1);
  });

  /**
   * The instance shown is the instance written to.
   *
   * Bound to an empty string, the select would match no option and render
   * blank while `declare` fell back to whichever instance loaded first. With
   * one Arr that fallback is always right and the blank reads as a cosmetic
   * nothing. With two, a destination would land on the wrong one with nothing
   * on screen to contradict it.
   */
  it('declares against the instance the form is showing', async () => {
    const declare = vi
      .spyOn(api, 'declareRootFolder')
      .mockResolvedValue({ id: 'rf-x', path: '/data/anime', verified: true });
    show([folder()], [], [], [instance('i1', 'Radarr'), instance('i2', 'Sonarr')]);

    const picker = (await screen.findByLabelText('Instance')) as HTMLSelectElement;
    // Seeded once the instances arrive, so the assertion waits for the load
    // rather than for the element, which the form renders before it.
    await waitFor(() => expect(picker.value).toBe('i1'));

    await userEvent.selectOptions(picker, 'i2');
    await userEvent.type(screen.getByLabelText('Destination folder'), '/data/anime');
    await fireEvent.click(screen.getByRole('button', { name: 'Add' }));

    await waitFor(() => expect(declare).toHaveBeenCalledWith('i2', '/data/anime'));
  });

  /**
   * Only what Routarr owns. A folder the instance reports would come back on
   * the next sync, without the category mapped onto it, so the interface does
   * not offer to remove it at all.
   */
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

  /**
   * A refusal is never read beside the previous success: the older, louder
   * sentence is the one that gets believed.
   */
  it('drops the previous success when the next attempt is refused', async () => {
    vi.spyOn(api, 'declareRootFolder')
      .mockResolvedValueOnce({ id: 'rf-x', path: '/ok', verified: true })
      .mockRejectedValueOnce(new Error('the instance cannot see it'));
    show([folder()], []);

    const field = await screen.findByLabelText('Destination folder');
    await userEvent.type(field, '/ok');
    await fireEvent.click(screen.getByRole('button', { name: 'Add' }));
    expect(await screen.findByText(/added/)).toBeInTheDocument();

    await userEvent.type(field, '/mnt/typo');
    await fireEvent.click(screen.getByRole('button', { name: 'Add' }));

    await waitFor(() => expect(screen.queryByText(/added/)).toBeNull());
    expect(await screen.findByText(/cannot see it/)).toBeInTheDocument();
  });

  /** The select reads what the server holds, never the choice it refused. */
  it('leaves the select on the stored category when a save is refused', async () => {
    vi.spyOn(api, 'updateRootFolderCategory').mockRejectedValue(
      new ApiError('anime already maps /data/other', 409, 'conflict'),
    );
    show(
      [folder({ path: '/data/anime', category: 'standard' })],
      [category(), category({ id: 'c2', name: 'anime' })],
    );
    const select = (await screen.findByLabelText('Category for /data/anime')) as HTMLSelectElement;

    await userEvent.selectOptions(select, 'anime');
    await fireEvent.click(screen.getByRole('button', { name: 'Save – /data/anime' }));

    expect(await screen.findByText('anime already maps /data/other')).toBeInTheDocument();
    await waitFor(() => expect(select.value).toBe('standard'));
  });

  /** A refused path stays in its field, to be corrected rather than typed again. */
  it('keeps a refused destination in its field', async () => {
    vi.spyOn(api, 'declareRootFolder').mockRejectedValue(new Error('the instance cannot see it'));
    show([folder()], []);
    const field = (await screen.findByLabelText('Destination folder')) as HTMLInputElement;

    await userEvent.type(field, '/mnt/typo');
    await fireEvent.click(screen.getByRole('button', { name: 'Add' }));

    expect(await screen.findByText(/cannot see it/)).toBeInTheDocument();
    // Once the screen has read the folders again, which is when a success
    // clears the field.
    await waitFor(() => expect(api.getRootFolders).toHaveBeenCalledTimes(2));
    await new Promise((resolve) => setTimeout(resolve, 0));
    expect(field.value).toBe('/mnt/typo');
  });

  /**
   * A refused name stays in its dialog, and the reason is said there: the page
   * banner sits under the modal, dimmed and out of reach.
   */
  it('keeps a refused new category in its dialog and says why there', async () => {
    vi.spyOn(api, 'createCategory').mockRejectedValue(
      new ApiError('A category named anime exists', 409, 'conflict'),
    );
    show([folder()], [category()]);

    await fireEvent.click(await screen.findByRole('button', { name: 'New category' }));
    const dialog = await screen.findByRole('dialog');
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

    expect(await answerConfirmation(null)).toBe('Remove the destination /data/anime?');
    expect(remove).not.toHaveBeenCalled();
  });

  it('asks before deleting a category, and deletes nothing on Cancel', async () => {
    const remove = vi.spyOn(api, 'deleteCategory');
    show([folder()], [category({ id: 'c2', name: 'kids', is_default: false })]);

    await fireEvent.click(await screen.findByRole('button', { name: 'Delete – kids' }));

    expect(await answerConfirmation(null)).toBe('Delete the category "kids"?');
    expect(remove).not.toHaveBeenCalled();
  });

  it('shows a refusal without waiting for the page to reload', async () => {
    vi.spyOn(api, 'declareRootFolder').mockRejectedValue(new Error('the instance cannot see it'));
    show([folder()], []);
    const field = await screen.findByLabelText('Destination folder');
    vi.spyOn(api, 'getRootFolders').mockReturnValue(new Promise(() => {}));

    await userEvent.type(field, '/mnt/typo');
    await fireEvent.click(screen.getByRole('button', { name: 'Add' }));

    expect(await screen.findByText(/cannot see it/)).toBeInTheDocument();
  });

  /**
   * An unmapped category yields `action = 'skip'`, never an error, so the
   * screen has to make "mapped to nothing" a deliberate, reachable choice
   * rather than a state you can only fall into.
   */
  it('sends null, not an empty string, when a folder is unmapped', async () => {
    const update = vi.spyOn(api, 'updateRootFolderCategory').mockResolvedValue(undefined as never);
    show([folder({ path: '/data/films' })], [category()]);

    await userEvent.selectOptions(await screen.findByLabelText('Category for /data/films'), '');
    await fireEvent.click(screen.getByRole('button', { name: 'Save – /data/films' }));

    await waitFor(() => expect(update).toHaveBeenCalledWith('rf1', null));
  });

  /** One banner a severity, as the dashboard draws its warnings. */
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

  /** Opened on its close button, a form is one reflex Enter from thrown away. */
  it('opens each category dialog on its name field', async () => {
    show([], [category({ name: 'anime' })]);

    await fireEvent.click(await screen.findByRole('button', { name: 'Rename category – anime' }));
    expect(document.activeElement).toBe(await screen.findByLabelText('Name'));
    await fireEvent.click(screen.getByRole('button', { name: 'Cancel' }));

    await fireEvent.click(await screen.findByRole('button', { name: 'New category' }));
    expect(document.activeElement).toBe(await screen.findByLabelText('Name'));
  });
});

/** A removal takes its row and the pressed button: the next row's takes the focus. */
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
    show([], [category(), next]);
    vi.spyOn(api, 'deleteCategory').mockResolvedValue(undefined as never);

    await fireEvent.click(await screen.findByRole('button', { name: 'Delete – standard' }));
    vi.spyOn(api, 'getCategories').mockResolvedValue([next]);
    await answerConfirmation();

    await waitFor(() =>
      expect(document.activeElement).toBe(screen.getByRole('button', { name: 'Delete – anime' })),
    );
  });
});
