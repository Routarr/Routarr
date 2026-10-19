import { describe, it, expect, vi, afterEach } from 'vitest';
import { fireEvent, screen, waitFor, within } from '@testing-library/svelte';
import userEvent from '@testing-library/user-event';

import { renderWithI18n } from '../test/render';
import { category, instance as fixtureInstance } from '../test/fixtures';
import { statusRevision } from '../lib/status.svelte';
import { api, ApiError } from '../api/client';
import type { Category, MappingConflict, RootFolder } from '../api/types';
import { answerConfirmation } from '../test/confirm';
import { unloading } from '../test/leaving';
import { navigate, router } from '../lib/router.svelte';
import Categories from './Categories.svelte';

/**
 * Categories are strings joined by name with no foreign key, so this screen keeps the names in
 * step. The API refuses to delete one in use, and the default one cannot be deleted at all.
 */

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
  // One instance is the usual install, and a second makes the form's choice observable.
  instances = [instance('i1', 'Radarr')],
) {
  vi.spyOn(api, 'getRootFolders').mockResolvedValue(folders);
  vi.spyOn(api, 'getCategories').mockResolvedValue(categories);
  vi.spyOn(api, 'getMappingConflicts').mockResolvedValue(conflicts);
  vi.spyOn(api, 'getInstances').mockResolvedValue(instances);
  return renderWithI18n(Categories);
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
    expect(screen.getByText('default')).toBeTruthy();
    expect(screen.queryByRole('button', { name: 'Delete – standard' })).toBeNull();
    expect(screen.getByRole('button', { name: 'Delete – anime' })).toBeTruthy();
  });

  /**
   * The server renames the row, the `%_category` columns and the setting in one transaction.
   * `{@const}` is reactive: clearing the dialog before reading the id would send no rename.
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
   * A category another folder of the instance holds is joined only once asked: the folders then
   * share its new titles. Each folder of the set names the others it shares with.
   */
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
    await fireEvent.click(screen.getByRole('button', { name: 'Save – /data/anime2' }));
    expect(await answerConfirmation(null)).toContain('/data/anime');
    expect(update).not.toHaveBeenCalled();

    await userEvent.selectOptions(picker, 'anime');
    await fireEvent.click(screen.getByRole('button', { name: 'Save – /data/anime2' }));
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

  /**
   * An arrow key on a closed select fires `change` on Windows and Linux, so writing on `change`
   * would send every category passed on the way.
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
    await waitFor(() => expect(update).toHaveBeenCalledWith('rf1', 'anime', false));
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
      'Leave without saving the categories picked for the folders? Unsaved changes: 1',
    );
    expect(router.path).not.toBe('/rules');
    expect(picker).toHaveValue('anime');
  });

  /**
   * A mapping can clear the "categories mapped to nothing" warning, which the shell counts, so
   * the screen tells it the answer changed.
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
   * A NAS that spins down is unreachable every night, so a folder that did not answer says when
   * it last did, as a warning: an error badge for a sleeping disk teaches the reader to ignore it.
   */
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

  /**
   * A target need not be a root folder in the Arr already: an operator keeps one root folder per
   * Arr and declares the targets beneath it here.
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
   * The instance shown is the instance written to. Bound to an empty string, the select would show
   * blank while `declare` used the first instance, which is wrong as soon as there are two.
   */
  it('declares against the instance the form is showing', async () => {
    const declare = vi
      .spyOn(api, 'declareRootFolder')
      .mockResolvedValue({ id: 'rf-x', path: '/data/anime', verified: true });
    show([folder()], [], [], [instance('i1', 'Radarr'), instance('i2', 'Sonarr')]);

    const picker = (await screen.findByLabelText('Instance')) as HTMLSelectElement;
    // Seeded once the instances load, after the form first renders.
    await waitFor(() => expect(picker.value).toBe('i1'));

    await userEvent.selectOptions(picker, 'i2');
    await userEvent.type(screen.getByLabelText('Destination folder'), '/data/anime');
    await fireEvent.click(screen.getByRole('button', { name: 'Add' }));

    await waitFor(() => expect(declare).toHaveBeenCalledWith('i2', '/data/anime'));
  });

  /**
   * A folder the instance reports would come back at the next sync without its category, so only
   * a declared one is offered for removal.
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
    // A success clears the field once the folders are read again, so wait for that read.
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

    expect(await answerConfirmation(null)).toBe(
      'Remove the destination /data/anime? A category mapped to it no longer routes there. Nothing is deleted on disk.',
    );
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
   * An unmapped category makes a decision `skip`, never an error, so the screen offers "mapped to
   * nothing" as a deliberate choice.
   */
  it('sends null, not an empty string, when a folder is unmapped', async () => {
    const update = vi.spyOn(api, 'updateRootFolderCategory').mockResolvedValue(undefined as never);
    show([folder({ path: '/data/films' })], [category()]);

    await userEvent.selectOptions(await screen.findByLabelText('Category for /data/films'), '');
    await fireEvent.click(screen.getByRole('button', { name: 'Save – /data/films' }));

    await waitFor(() => expect(update).toHaveBeenCalledWith('rf1', null, false));
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
