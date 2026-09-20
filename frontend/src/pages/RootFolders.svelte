<script lang="ts">
  import { AlertTriangle, Pencil, Plus, Trash2 } from '../lib/icons';
  import { api } from '../api/client';
  import type { Category, Instance, MappingConflict, RootFolder } from '../api/types';
  import { formatBytes, formatRelative } from '../api/format';
  import { createAsync, describeError } from '../lib/async.svelte';
  import { i18n, t } from '../lib/i18n.svelte';
  import EmptyState from '../components/EmptyState.svelte';
  import ErrorBanner from '../components/ErrorBanner.svelte';
  import Loading from '../components/Loading.svelte';
  import Modal from '../components/Modal.svelte';
  import SuccessBanner from '../components/SuccessBanner.svelte';
  import TableRegion from '../components/TableRegion.svelte';
  import { invalidateStatus } from '../lib/status.svelte';

  const bundle = createAsync(async () => {
    const [folders, categories, conflicts, instances] = await Promise.all([
      api.getRootFolders(),
      api.getCategories(),
      api.getMappingConflicts(),
      api.getInstances(),
    ]);
    return { folders, categories, conflicts, instances };
  });

  let notice = $state<string | null>(null);
  let creating = $state(false);
  /** The destination being declared: an instance and a path it can see. */
  let target = $state('');
  let targetPath = $state('');
  let name = $state('');
  let description = $state('');

  const folders = $derived<RootFolder[]>(bundle.data?.folders ?? []);
  const categories = $derived<Category[]>(bundle.data?.categories ?? []);
  const conflicts = $derived<MappingConflict[]>(bundle.data?.conflicts ?? []);
  const instances = $derived<Instance[]>(bundle.data?.instances ?? []);

  /**
   * The select must show the instance the declaration will use.
   *
   * Bound to an empty string it matches no option, so it renders blank — and a
   * blank select beside a fallback to `instances[0]` means that with two Arrs a
   * destination is declared against one the operator never chose and the screen
   * never named.
   */
  $effect(() => {
    if (!instances.some((instance) => instance.id === target)) target = instances[0]?.id ?? '';
  });

  let renaming = $state<Category | null>(null);
  let newName = $state('');

  async function act(fn: () => Promise<unknown>, message: string) {
    bundle.error = null;
    // The previous success goes with the previous attempt. Left standing, a
    // refused action reads as "Destination added" beside the reason it was
    // not — and the older, louder sentence is the one that gets believed.
    notice = null;
    try {
      await fn();
      notice = message;
      // Mapping a category, or removing one, is what clears the two warnings
      // about categories and instances that reach no folder.
      invalidateStatus();
      await bundle.reload();
    } catch (err) {
      // Reloaded first, then told. `reload` clears `error` on entry, so setting
      // it before was wiped by the very reload meant to refresh the rejected
      // state — and every refusal on this screen passed in silence.
      await bundle.reload();
      bundle.error = describeError(err);
    }
  }

  /**
   * Declare a destination the instance does not report as a root folder.
   *
   * A target used to have to exist in Radarr or Sonarr already, so routing into
   * `/movies/anime` meant declaring it there first. The arrangement an operator
   * wants is the opposite: one root folder per Arr, and the targets beneath it
   * named here.
   */
  async function declare(event: SubmitEvent) {
    event.preventDefault();
    if (!target || !targetPath.trim()) return;
    await act(
      () => api.declareRootFolder(target, targetPath.trim()),
      t('DestinationDeclared', { path: targetPath.trim() }),
    );
    targetPath = '';
  }

  async function createCategory(event: SubmitEvent) {
    event.preventDefault();
    await act(
      () => api.createCategory({ name, description: description || null }),
      t('CategoryCreated', { name: name.trim().toLowerCase() }),
    );
    name = '';
    description = '';
    creating = false;
  }
</script>

<div>
  <div class="page-header">
    <div>
      <h1 class="page-title">{t('RootFolders')}</h1>
      <p class="page-subtitle">{t('RootFoldersSubtitle')}</p>
    </div>
    <button class="btn btn-primary" onclick={() => (creating = true)}>
      <Plus size={16} />
      {t('NewCategory')}
    </button>
  </div>

  <ErrorBanner
    message={bundle.error}
    onDismiss={() => (bundle.error = null)}
    onRetry={() => void bundle.reload()}
  />
  <SuccessBanner message={notice} />

  {#each conflicts as conflict, index (index)}
    <div class="banner {conflict.severity === 'error' ? 'banner-danger' : 'banner-warning'}">
      <AlertTriangle size={16} />
      <span>
        {#if conflict.instance_name}<strong>{conflict.instance_name}: </strong>{/if}
        {conflict.message}
      </span>
    </div>
  {/each}

  <div class="card">
    <div class="card-header">
      <h2 class="card-title">{t('FolderMappings')}</h2>
    </div>
    <TableRegion label={t('FolderMappings')}>
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
            <tr><td colspan="7"><Loading /></td></tr>
          {:else if folders.length === 0}
            <tr><td colspan="7"><EmptyState>{t('NoRootFolderDiscovered')}</EmptyState></td></tr>
          {:else}
            {#each folders as folder (folder.id)}
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
                       which — "20 minutes ago" reads as a nap and "3 days ago"
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
                  <select
                    class="form-select"
                    aria-label={t('CategoryForFolder', { path: folder.path })}
                    value={folder.category ?? ''}
                    onchange={(event) =>
                      act(
                        () =>
                          api.updateRootFolderCategory(
                            folder.id,
                            event.currentTarget.value || null,
                          ),
                        t('MappingUpdated'),
                      )}
                  >
                    <option value="">{t('Unmapped')}</option>
                    {#each categories as category (category.id)}
                      <option value={category.name}>{category.name}</option>
                    {/each}
                  </select>
                </td>
                <td>
                  <!-- Only what Routarr owns. A folder the instance reports
                       would come back on the next sync, without its category. -->
                  {#if folder.origin === 'declared'}
                    <button
                      class="btn btn-danger btn-sm"
                      type="button"
                      aria-label="{t('Remove')} — {folder.path}"
                      onclick={() =>
                        act(
                          () => api.deleteRootFolder(folder.id),
                          t('DestinationRemoved', { path: folder.path }),
                        )}
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
      <div class="form-group flex-1">
        <label class="form-label" for="declare-path">{t('DeclareDestination')}</label>
        <input
          id="declare-path"
          class="form-input"
          bind:value={targetPath}
          placeholder={t('DeclareDestinationPlaceholder')}
        />
      </div>
      <button class="btn btn-secondary" type="submit" disabled={!targetPath.trim() || !target}>
        {t('AddDestination')}
      </button>
    </form>
    <p class="text-muted text-sm mt-1">{t('DeclareDestinationHelp')}</p>
  </div>

  <div class="card">
    <div class="card-header">
      <h2 class="card-title">{t('Categories')}</h2>
    </div>
    <TableRegion label={t('Categories')}>
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
          {#each categories as category (category.id)}
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
              <td>
                <span
                  class="badge {category.root_folder_count > 0 ? 'badge-success' : 'badge-warning'}"
                >
                  {category.root_folder_count}
                </span>
              </td>
              <td>
                <div class="flex gap-2">
                  <button
                    class="btn btn-secondary btn-sm"
                    aria-label="{t('RenameCategory')} — {category.name}"
                    title={t('RenameCategory')}
                    onclick={() => {
                      renaming = category;
                      newName = category.name;
                    }}
                  >
                    <Pencil size={14} />
                  </button>
                  {#if !category.is_default}
                    <button
                      class="btn btn-danger btn-sm"
                      aria-label="{t('Delete')} — {category.name}"
                      title={t('Delete')}
                      onclick={() =>
                        act(() => api.deleteCategory(category.id), t('CategoryDeleted'))}
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
    <Modal label={t('RenameCategory')} onClose={() => (renaming = null)}>
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
      <!-- `novalidate`, like the form below and the rule editor: the browser's
           bubble speaks the browser's language, not `ui_language`. Save is held
           until the field is filled, so the constraint is enforced before the
           press rather than complained about after it. -->
      <form
        novalidate
        onsubmit={(event) => {
          event.preventDefault();
          // Read before closing. `{@const}` is reactive, so `target` follows
          // `renaming` — clearing it first and reading `target.id` afterwards
          // reads it as null, and the rename never leaves the browser.
          const id = target.id;
          const name = newName;
          renaming = null;
          void act(() => api.renameCategory(id, name), t('CategoryRenamed'));
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
        <div class="flex justify-between mt-4">
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
    <Modal label={t('NewCategory')} onClose={() => (creating = false)}>
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
      <!-- `novalidate`: the browser's own bubble renders in the *browser's*
       language whatever `ui_language` says, and fires before the submit
       handler. Nothing is traded away for it — Save is held until the required
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
        <div class="flex justify-between mt-4">
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
