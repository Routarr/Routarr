<script lang="ts">
  import { Pencil, Plus, Trash2 } from '../lib/icons';
  import { api } from '../api/client';
  import type { Category, Instance, MappingConflict, RootFolder } from '../api/types';
  import { formatBytes, formatRelative, isolated } from '../api/format';
  import { createAsync, describeError } from '../lib/async.svelte';
  import { askConfirmation } from '../lib/confirm.svelte';
  import { createOutcome } from '../lib/outcome.svelte';
  import { i18n, t } from '../lib/i18n.svelte';
  import Count from '../components/Count.svelte';
  import EmptyState from '../components/EmptyState.svelte';
  import ErrorBanner from '../components/ErrorBanner.svelte';
  import Modal from '../components/Modal.svelte';
  import OutcomeBanner from '../components/OutcomeBanner.svelte';
  import GuideStepBanner from '../components/GuideStepBanner.svelte';
  import TableSkeleton from '../components/TableSkeleton.svelte';
  import BannerList from '../components/BannerList.svelte';
  import TableRegion from '../components/TableRegion.svelte';
  import { invalidateStatus } from '../lib/status.svelte';
  import { handFocus } from '../lib/focus';
  import { holdUnsaved } from '../lib/unsaved.svelte';

  const bundle = createAsync(async (signal) => {
    const [folders, categories, conflicts, instances] = await Promise.all([
      api.getRootFolders(signal),
      api.getCategories(signal),
      api.getMappingConflicts(signal),
      api.getInstances(signal),
    ]);
    return { folders, categories, conflicts, instances };
  });

  const outcome = createOutcome();
  let creating = $state(false);
  /** The destination being declared: an instance and a path it can see. */
  let target = $state('');
  let targetPath = $state('');
  let name = $state('');
  let description = $state('');

  const folders = $derived<RootFolder[]>(bundle.data?.folders ?? []);
  const categories = $derived<Category[]>(bundle.data?.categories ?? []);
  const conflicts = $derived<MappingConflict[]>(bundle.data?.conflicts ?? []);
  const errors = $derived(conflicts.filter((conflict) => conflict.severity === 'error'));
  const warnings = $derived(conflicts.filter((conflict) => conflict.severity !== 'error'));
  const instances = $derived<Instance[]>(bundle.data?.instances ?? []);

  /**
   * The select must show the instance the declaration will use.
   *
   * Bound to an empty string it matches no option, so it renders blank, and a
   * blank select beside a fallback to `instances[0]` means that with two Arrs a
   * destination is declared against one the operator never chose and the screen
   * never named.
   */
  $effect(() => {
    if (!instances.some((instance) => instance.id === target)) target = instances[0]?.id ?? '';
  });

  let renaming = $state<Category | null>(null);
  let newName = $state('');

  /**
   * The category picked for each folder, by folder id, until Save writes it.
   * On Windows and Linux an arrow key on a closed select fires `change`, and
   * writing on `change` would send every category passed on the way to the
   * server.
   */
  const picked = $state<Record<string, string>>({});
  const pickedFor = (folder: RootFolder) => picked[folder.id] ?? folder.category ?? '';
  const unsavedPicks = $derived(
    folders.filter((folder) => pickedFor(folder) !== (folder.category ?? '')).length,
  );
  holdUnsaved(
    () => unsavedPicks > 0,
    () => t('ConfirmLeaveUnsavedMappings', { count: unsavedPicks }),
  );

  async function saveCategory(folder: RootFolder) {
    const category = pickedFor(folder);
    await act(() => api.updateRootFolderCategory(folder.id, category || null), t('MappingUpdated'));
    // Written or refused, the row reads what the server now holds.
    delete picked[folder.id];
    // Save turns disabled once nothing is pending, and a disabled button
    // drops the focus: the folder's select takes it back.
    void handFocus(`folder-${folder.id}-category`);
  }

  /** Whether the write went through: what was typed is cleared on success only. */
  async function act(fn: () => Promise<unknown>, message: string): Promise<boolean> {
    try {
      await fn();
      outcome.succeed(message);
      // Mapping a category, or removing one, is what clears the two warnings
      // about categories and instances that reach no folder.
      invalidateStatus();
      await bundle.reload();
      return true;
    } catch (err) {
      outcome.fail(err);
      await bundle.reload();
      return false;
    }
  }

  /**
   * A write from a dialog, whose refusal is said inside it: the page banner
   * sits under the modal, dimmed and out of reach. The dialog closes on
   * success only, so a refused name is corrected rather than typed again.
   */
  let dialogError = $state<string | null>(null);
  async function fromDialog(fn: () => Promise<unknown>, message: string): Promise<boolean> {
    dialogError = null;
    try {
      await fn();
    } catch (err) {
      dialogError = describeError(err);
      return false;
    }
    outcome.succeed(message);
    invalidateStatus();
    await bundle.reload();
    return true;
  }

  // A removed destination or category takes its row and the pressed button
  // with it: the same button on the row now in its place takes the focus, else
  // the one before, else the table.
  const removeId = (index: number) => `folders-remove-${index}`;
  const deleteCategoryId = (index: number) => `categories-delete-${index}`;

  /**
   * Declare a destination the instance does not report as a root folder: one
   * root folder per Arr is what an operator keeps there, and the targets
   * beneath it are named here.
   */
  let declaring = $state(false);

  async function declare(event: SubmitEvent) {
    event.preventDefault();
    if (!target || !targetPath.trim() || declaring) return;
    const pressed = document.activeElement as HTMLElement | null;
    declaring = true;
    try {
      const declared = await act(
        () => api.declareRootFolder(target, targetPath.trim()),
        t('DestinationDeclared', { path: targetPath.trim() }),
      );
      if (declared) targetPath = '';
    } finally {
      declaring = false;
      // Add stays disabled once its path is cleared, so the next path takes the focus.
      void handFocus(pressed, 'declare-path');
    }
  }

  async function createCategory(event: SubmitEvent) {
    event.preventDefault();
    const created = await fromDialog(
      () => api.createCategory({ name, description: description || null }),
      t('CategoryCreated', { name: name.trim().toLowerCase() }),
    );
    if (!created) return;
    name = '';
    description = '';
    creating = false;
  }

  function openDialog(open: () => void) {
    dialogError = null;
    open();
  }
</script>

<div>
  <div class="page-header">
    <div>
      <h1 class="page-title">{t('RootFolders')}</h1>
      <p class="page-subtitle">{t('RootFoldersSubtitle')}</p>
    </div>
    <div class="flex gap-2">
      <button class="btn btn-primary" onclick={() => openDialog(() => (creating = true))}>
        <Plus size={16} />
        {t('NewCategory')}
      </button>
    </div>
  </div>

  <ErrorBanner
    message={bundle.error}
    onDismiss={() => (bundle.error = null)}
    onRetry={() => void bundle.reload()}
  />
  <OutcomeBanner {outcome} />
  <GuideStepBanner step="categories" />

  <BannerList
    tone="danger"
    title={t('DiagnosticErrors', { count: errors.length })}
    items={errors.map((conflict) => ({ lead: conflict.instance_name, text: conflict.message }))}
  />
  <BannerList
    tone="warning"
    title={t('DiagnosticWarnings', { count: warnings.length })}
    items={warnings.map((conflict) => ({ lead: conflict.instance_name, text: conflict.message }))}
  />

  <div class="card">
    <div class="card-header">
      <h2 class="card-title">{t('FolderMappings')}</h2>
    </div>
    <TableRegion label={t('FolderMappings')} id="folders-table">
      <table>
        <caption class="visually-hidden">{t('FolderMappings')}</caption>
        <thead>
          <tr>
            <th>{t('Instance')}</th>
            <th>{t('Type')}</th>
            <th>{t('Path')}</th>
            <th>{t('FreeSpace')}</th>
            <th>{t('State')}</th>
            <th class="w-220">{t('Category')}</th>
            <th class="w-80"><span class="visually-hidden">{t('Actions')}</span></th>
          </tr>
        </thead>
        <tbody>
          {#if bundle.loading && folders.length === 0}
            <TableSkeleton columns={7} />
          {:else if folders.length === 0 && !bundle.error}
            <tr><td colspan="7"><EmptyState>{t('NoRootFolderDiscovered')}</EmptyState></td></tr>
          {:else}
            {#each folders as folder, index (folder.id)}
              <tr>
                <td><strong>{folder.instance_name}</strong></td>
                <td>
                  <span class="badge badge-kind kind-{folder.instance_type}">
                    {folder.instance_type}
                  </span>
                </td>
                <td class="mono">{folder.path}</td>
                <td class="text-muted">{formatBytes(folder.free_space, i18n.language)}</td>
                <td>
                  <!-- A warning, not an error, and stated with its date: a NAS
                       that spins down is reported unreachable every night, and
                       an error badge for a disk that is merely asleep is how a
                       diagnostic gets ignored. No threshold decides which is
                       which: "20 minutes ago" reads as a nap and "3 days ago"
                       as a fault, and the reader knows their own hardware. -->
                  <span class="badge {folder.accessible ? 'badge-success' : 'badge-warning'}">
                    {t(folder.accessible ? 'Accessible' : 'Inaccessible')}
                  </span>
                  {#if !folder.accessible}
                    <div class="text-muted text-sm mt-1">
                      {folder.last_accessible_at
                        ? t('LastAnswered', {
                            since: formatRelative(folder.last_accessible_at, i18n.language),
                          })
                        : t('NeverAnswered')}
                    </div>
                  {/if}
                </td>
                <td>
                  <div class="flex gap-2">
                    <select
                      id="folder-{folder.id}-category"
                      class="form-select"
                      aria-label={t('CategoryForFolder', { path: folder.path })}
                      value={pickedFor(folder)}
                      onchange={(event) => (picked[folder.id] = event.currentTarget.value)}
                    >
                      <option value="">{t('Unmapped')}</option>
                      {#each categories as category (category.id)}
                        <option value={category.name}>{category.name}</option>
                      {/each}
                    </select>
                    <button
                      type="button"
                      class="btn btn-secondary btn-sm"
                      disabled={pickedFor(folder) === (folder.category ?? '')}
                      aria-label="{t('Save')} – {folder.path}"
                      onclick={() => void saveCategory(folder)}
                    >
                      {t('Save')}
                    </button>
                  </div>
                </td>
                <td>
                  <!-- Only what Routarr owns. A folder the instance reports
                       would come back on the next sync, without its category. -->
                  {#if folder.origin === 'declared'}
                    <button
                      id={removeId(index)}
                      class="btn btn-danger btn-sm"
                      type="button"
                      aria-label="{t('Remove')} – {folder.path}"
                      onclick={async () => {
                        if (
                          await askConfirmation(
                            t('ConfirmRemoveDestination', { path: isolated(folder.path) }),
                            'Remove',
                          )
                        ) {
                          await act(
                            () => api.deleteRootFolder(folder.id),
                            t('DestinationRemoved', { path: folder.path }),
                          );
                          void handFocus(removeId(index), removeId(index - 1), 'folders-table');
                        }
                      }}
                    >
                      {t('Remove')}
                    </button>
                  {/if}
                </td>
              </tr>
            {/each}
          {/if}
        </tbody>
      </table>
    </TableRegion>

    <!-- Under the table it adds to, not on a screen of its own: what an
         operator is doing here is completing the list above. -->
    <form novalidate class="form-row mt-4" onsubmit={declare}>
      <div class="form-group">
        <label class="form-label" for="declare-instance">{t('Instance')}</label>
        <select id="declare-instance" class="form-select" bind:value={target}>
          {#each instances as instance (instance.id)}
            <option value={instance.id}>{instance.name}</option>
          {/each}
        </select>
      </div>
      <!-- A base width, so on a phone the path takes a line of its own rather
           than folding its caption beside the instance picker. -->
      <div class="form-group flex-fill-240">
        <label class="form-label" for="declare-path">{t('DeclareDestination')}</label>
        <input
          id="declare-path"
          class="form-input mono"
          bind:value={targetPath}
          placeholder={t('DeclareDestinationPlaceholder')}
        />
      </div>
      <button
        class="btn btn-secondary"
        type="submit"
        disabled={!targetPath.trim() || !target || declaring}
      >
        {declaring ? t('Saving') : t('AddDestination')}
      </button>
    </form>
    <p class="text-muted text-sm mt-1">{t('DeclareDestinationHelp')}</p>
  </div>

  <div class="card">
    <div class="card-header">
      <h2 class="card-title">{t('Categories')}</h2>
    </div>
    <TableRegion label={t('Categories')} id="categories-table">
      <table>
        <caption class="visually-hidden">{t('Categories')}</caption>
        <thead>
          <tr>
            <th>{t('Name')}</th>
            <th>{t('Description')}</th>
            <th>{t('RulesUsingIt')}</th>
            <th>{t('FoldersMapped')}</th>
            <th class="w-80"><span class="visually-hidden">{t('Actions')}</span></th>
          </tr>
        </thead>
        <tbody>
          {#if bundle.loading && categories.length === 0}
            <TableSkeleton columns={5} />
          {/if}
          {#each categories as category, index (category.id)}
            <tr>
              <td>
                <strong>{category.name}</strong>
                {#if category.is_default}
                  <span class="badge badge-value muted ms-2">
                    {t('DefaultCategoryBadge')}
                  </span>
                {/if}
              </td>
              <td class="text-muted">{category.description ?? t('None')}</td>
              <td>{category.rule_count}</td>
              <td><Count value={category.root_folder_count} /></td>
              <td>
                <div class="flex gap-2">
                  <button
                    class="btn btn-secondary btn-sm"
                    aria-label="{t('RenameCategory')} – {category.name}"
                    title={t('RenameCategory')}
                    onclick={() =>
                      openDialog(() => {
                        renaming = category;
                        newName = category.name;
                      })}
                  >
                    <Pencil size={14} />
                  </button>
                  {#if !category.is_default}
                    <button
                      id={deleteCategoryId(index)}
                      class="btn btn-danger btn-sm"
                      aria-label="{t('Delete')} – {category.name}"
                      title={t('Delete')}
                      onclick={async () => {
                        if (
                          await askConfirmation(
                            t('ConfirmDeleteCategory', { name: category.name }),
                            'Delete',
                          )
                        ) {
                          await act(() => api.deleteCategory(category.id), t('CategoryDeleted'));
                          void handFocus(
                            deleteCategoryId(index),
                            deleteCategoryId(index - 1),
                            'categories-table',
                          );
                        }
                      }}
                    >
                      <Trash2 size={14} />
                    </button>
                  {/if}
                </div>
              </td>
            </tr>
          {/each}
        </tbody>
      </table>
    </TableRegion>
  </div>

  {#if renaming}
    {@const target = renaming}
    <Modal
      label={t('RenameCategory')}
      onClose={() => (renaming = null)}
      initialFocus="rootfolders-rename"
    >
      <div class="modal-header">
        <h2 class="modal-title">{t('RenameCategory')}</h2>
        <button
          class="btn btn-secondary btn-sm"
          onclick={() => (renaming = null)}
          aria-label={t('Dismiss')}
          title={t('Dismiss')}
        >
          ✕
        </button>
      </div>
      <ErrorBanner message={dialogError} onDismiss={() => (dialogError = null)} />
      <!-- `novalidate`, like the form below and the rule editor: the browser's
           bubble speaks the browser's language, not `ui_language`. Save is held
           until the field is filled, so the constraint is enforced before the
           press rather than complained about after it. -->
      <form
        novalidate
        onsubmit={async (event) => {
          event.preventDefault();
          // Read before the request: `{@const}` is reactive, so `target`
          // follows `renaming`, which a success clears.
          const id = target.id;
          const renamed = await fromDialog(
            () => api.renameCategory(id, newName),
            t('CategoryRenamed'),
          );
          if (renamed) renaming = null;
        }}
      >
        <div class="form-group">
          <label class="form-label" for="rootfolders-rename">{t('Name')}</label>
          <input
            id="rootfolders-rename"
            class="form-input"
            placeholder={t('CategoryNamePlaceholder')}
            bind:value={newName}
            required
          />
          <p class="text-muted text-sm mt-1">
            {t('RenameCategoryHint')}
          </p>
        </div>
        <div class="dialog-actions">
          <button type="button" class="btn btn-secondary" onclick={() => (renaming = null)}>
            {t('Cancel')}
          </button>
          <button type="submit" class="btn btn-primary" disabled={!newName.trim()}
            >{t('Save')}</button
          >
        </div>
      </form>
    </Modal>
  {/if}

  {#if creating}
    <Modal
      label={t('NewCategory')}
      onClose={() => (creating = false)}
      initialFocus="rootfolders-name"
    >
      <div class="modal-header">
        <h2 class="modal-title">{t('NewCategory')}</h2>
        <button
          class="btn btn-secondary btn-sm"
          onclick={() => (creating = false)}
          aria-label={t('Dismiss')}
          title={t('Dismiss')}
        >
          ✕
        </button>
      </div>
      <ErrorBanner message={dialogError} onDismiss={() => (dialogError = null)} />
      <!-- `novalidate`: the browser's own bubble renders in the *browser's*
       language whatever `ui_language` says, and fires before the submit
       handler. Nothing is traded away for it: Save is held until the required
       fields are filled, so the constraint is enforced before the press rather
       than complained about after it. -->
      <form novalidate onsubmit={createCategory}>
        <div class="form-group">
          <label class="form-label" for="rootfolders-name">{t('Name')}</label>
          <input
            id="rootfolders-name"
            class="form-input"
            placeholder={t('CategoryNamePlaceholder')}
            bind:value={name}
            required
          />
          <p class="text-muted text-sm mt-1">{t('CategoryNameHint')}</p>
        </div>
        <div class="form-group">
          <label class="form-label" for="rootfolders-description">
            {t('Description')} ({t('Optional')})
          </label>
          <input id="rootfolders-description" class="form-input" bind:value={description} />
        </div>
        <div class="dialog-actions">
          <button type="button" class="btn btn-secondary" onclick={() => (creating = false)}>
            {t('Cancel')}
          </button>
          <button type="submit" class="btn btn-primary" disabled={!name.trim()}
            >{t('Create')}</button
          >
        </div>
      </form>
    </Modal>
  {/if}
</div>
