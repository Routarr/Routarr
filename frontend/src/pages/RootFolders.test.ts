import { describe, it, expect, vi, afterEach } from 'vitest';
import { fireEvent, screen, waitFor } from '@testing-library/svelte';
import userEvent from '@testing-library/user-event';

import { renderWithI18n } from '../test/render';
import { statusRevision } from '../lib/status.svelte';
import { api } from '../api/client';
import type { Category, MappingConflict, RootFolder } from '../api/types';
import RootFolders from './RootFolders.svelte';

/**
 * Categories are free-form strings joined by name, with no foreign key, so this
 * screen is where the names in four tables are kept in step. Deleting one that
 * is in use is refused by the API precisely because the database would not stop
 * it — and the *default* category cannot be deleted at all, since an unmatched
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
  RootFolders: 'Root folders',
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
  None: '—',
  Accessible: 'accessible',
  Inaccessible: 'unreachable',
  LastAnswered: 'Last answered {since}',
  NeverAnswered: 'Has never answered',
  NoRootFolderDiscovered: 'No root folder',
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

function instance(id: string, name: string) {
  return {
    id,
    name,
    instance_type: 'radarr',
    base_url: `http://${name}:7878`,
    api_key: '••••',
    enabled: true,
    sync_interval_minutes: 60,
    last_sync_at: null,
    last_sync_status: null,
    webhook_token: 'tok',
  } as never;
}

function show(
  folders: RootFolder[],
  categories: Category[],
  conflicts: MappingConflict[] = [],
  // The declaration form needs somewhere to send a path. One instance is the
  // ordinary install; the second is what makes the choice observable.
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
    expect(screen.queryByRole('button', { name: 'Delete — standard' })).toBeNull();
    // Every other category is deletable.
    expect(screen.getByRole('button', { name: 'Delete — anime' })).toBeTruthy();
  });

  it('names each row action after its category', async () => {
    show([], [category({ name: 'anime' })]);

    expect(await screen.findByRole('button', { name: 'Rename category — anime' })).toBeTruthy();
    expect(screen.getByRole('button', { name: 'Delete — anime' })).toBeTruthy();
  });

  /**
   * Nothing cascades: renaming updates the row, the four `%_category` columns
   * and the `default_category` setting, in one transaction on the server. The
   * screen's job is to send the new name and then re-read everything.
   *
   * `{@const}` is reactive, so clearing the dialog's state before reading the id
   * reads it as null and the rename never leaves the browser — which no compiler
   * sees.
   */
  it('renames through the API and re-reads what the rename touched', async () => {
    const rename = vi.spyOn(api, 'renameCategory').mockResolvedValue(undefined as never);
    const reread = vi.spyOn(api, 'getCategories');
    show([], [category({ name: 'anime' })]);

    await fireEvent.click(await screen.findByRole('button', { name: 'Rename category — anime' }));
    const field = await screen.findByLabelText('Name');
    await fireEvent.input(field, { target: { value: 'animation' } });
    await fireEvent.submit(field.closest('form') as HTMLFormElement);

    await waitFor(() => expect(rename).toHaveBeenCalledWith('c1', 'animation'));
    await waitFor(() => expect(reread.mock.calls.length).toBeGreaterThan(1));
  });

  it('maps a folder to the category picked on its own row', async () => {
    const update = vi.spyOn(api, 'updateRootFolderCategory').mockResolvedValue(undefined as never);
    show([folder({ path: '/data/anime', category: null })], [category({ name: 'anime' })]);

    await userEvent.selectOptions(
      await screen.findByLabelText('Category for /data/anime'),
      'anime',
    );

    await waitFor(() => expect(update).toHaveBeenCalledWith('rf1', 'anime'));
  });

  /**
   * Mapping a category is what clears the "categories mapped to nothing"
   * warning, and the shell counts that warning in two places. It owns the
   * request; this screen's job is to say that the answer changed.
   */
  it('tells the shell its counters are out of date', async () => {
    vi.spyOn(api, 'updateRootFolderCategory').mockResolvedValue(undefined as never);
    show([folder({ path: '/data/anime', category: null })], [category({ name: 'anime' })]);

    const before = statusRevision();
    await userEvent.selectOptions(
      await screen.findByLabelText('Category for /data/anime'),
      'anime',
    );

    await waitFor(() => expect(statusRevision()).toBeGreaterThan(before));
  });

  /**
   * An unmapped category yields `action = 'skip'`, never an error — so the
   * screen has to make "mapped to nothing" a deliberate, reachable choice
   * rather than a state you can only fall into.
   */
  /**
   * A NAS that spins down is reported unreachable every night, so a folder that
   * did not answer is stated with *when* it last did rather than as a fault: an
   * error badge for a disk that is merely asleep is how a diagnostic gets
   * ignored. No threshold decides which is which — the reader knows their own
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
   * A target used to have to be a root folder in Radarr or Sonarr already, so
   * routing into `/media/movies/anime` meant declaring it there first. What an
   * operator wants is one root folder per Arr and the targets beneath it named
   * here.
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

  /**
   * The instance shown is the instance written to.
   *
   * The select was bound to an empty string, which matches no option: it
   * rendered blank while `declare` fell back to whichever instance loaded
   * first. With one Arr that fallback is always right and the blank reads as a
   * cosmetic nothing; with two, a destination lands on the wrong one and the
   * screen showed nothing to contradict it.
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
   * the next sync, without the category mapped onto it — so the interface does
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

  it('shows a refusal without waiting for the page to reload', async () => {
    vi.spyOn(api, 'declareRootFolder').mockRejectedValue(new Error('the instance cannot see it'));
    show([folder()], []);
    const field = await screen.findByLabelText('Destination folder');
    vi.spyOn(api, 'getRootFolders').mockReturnValue(new Promise(() => {}));

    await userEvent.type(field, '/mnt/typo');
    await fireEvent.click(screen.getByRole('button', { name: 'Add' }));

    expect(await screen.findByText(/cannot see it/)).toBeInTheDocument();
  });

  it('sends null, not an empty string, when a folder is unmapped', async () => {
    const update = vi.spyOn(api, 'updateRootFolderCategory').mockResolvedValue(undefined as never);
    show([folder({ path: '/data/films' })], [category()]);

    await userEvent.selectOptions(await screen.findByLabelText('Category for /data/films'), '');

    await waitFor(() => expect(update).toHaveBeenCalledWith('rf1', null));
  });

  it('reports a folder the Arr cannot reach', async () => {
    show([folder({ accessible: false })], [category()]);

    expect(await screen.findByText('unreachable')).toBeTruthy();
  });

  it('surfaces a mapping conflict the server found', async () => {
    show(
      [folder()],
      [category()],
      [
        {
          kind: 'duplicate',
          severity: 'error',
          instance_name: 'Radarr',
          category: 'anime',
          message: 'Two folders claim “anime”',
        },
      ],
    );

    expect(await screen.findByText('Two folders claim “anime”')).toBeTruthy();
  });
});
