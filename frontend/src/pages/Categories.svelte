<script lang="ts">
  import CardTitle from '../components/CardTitle.svelte';
  import FolderSuggestions from '../components/FolderSuggestions.svelte';
  import { Pencil, Plus, RefreshCw, Save, Trash2 } from '../lib/icons';
  import { api } from '../api/client';
  import type { Category, Instance, MappingConflict, RootFolder } from '../api/types';
  import { ARR_NAME, formatBytes, formatRelative, instanceKindKey } from '../api/format';
  import { createAsync, describeError } from '../lib/async.svelte';
  import { ask, askConfirmation } from '../lib/confirm.svelte';
  import { createOutcome } from '../lib/outcome.svelte';
  import { i18n, t } from '../lib/i18n.svelte';
  import NoValue from '../components/NoValue.svelte';
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
  import { access } from '../lib/access.svelte';
  import { poll } from '../lib/poll.svelte';

  const bundle = createAsync(async (signal) => {
    const [folders, categories, conflicts, instances, suggestions] = await Promise.all([
      api.getRootFolders(signal),
      api.getCategories(signal),
      api.getMappingConflicts(signal),
      api.getInstances(signal),
      // The owner's to accept: a reader is refused the route.
      access.readOnly ? [] : api.getFolderSuggestions(signal),
    ]);
    return { folders, categories, conflicts, instances, suggestions };
  });

  // Its relative times follow on their own while it is open, the picks kept apart from the load.
  poll(
    () => void bundle.reload(),
    () => 60_000,
  );

  const outcome = createOutcome();
  let creating = $state(false);
  /** The destination typed under each instance's folders, by instance id. */
  const targetPaths = $state<Record<string, string>>({});
  let name = $state('');
  let description = $state('');

  const folders = $derived<RootFolder[]>(bundle.data?.folders ?? []);
  const categories = $derived<Category[]>(bundle.data?.categories ?? []);
  const conflicts = $derived<MappingConflict[]>(bundle.data?.conflicts ?? []);
  const errors = $derived(conflicts.filter((conflict) => conflict.severity === 'error'));
  const warnings = $derived(conflicts.filter((conflict) => conflict.severity !== 'error'));
  const instances = $derived<Instance[]>(bundle.data?.instances ?? []);

  /**
   * The folders of each instance under it, an instance with none included: its warnings and its
   * first sync land there. A folder whose instance the list lacks keeps a group of its own.
   */
  const groups = $derived.by(() => {
    const listed = instances.map((instance) => ({
      id: instance.id,
      name: instance.name,
      type: instance.instance_type as string,
      instance: instance as Instance | null,
    }));
    for (const folder of folders) {
      if (listed.some((group) => group.id === folder.instance_id)) continue;
      listed.push({
        id: folder.instance_id,
        name: folder.instance_name,
        type: folder.instance_type,
        instance: null,
      });
    }
    return listed.map((group) => ({
      ...group,
      folders: folders.filter((folder) => folder.instance_id === group.id),
    }));
  });

  let renaming = $state<Category | null>(null);
  let newName = $state('');

  /**
   * The category picked for each folder, by folder id, until the save bar writes it: an arrow
   * key on a closed select fires `change` on Windows and Linux.
   */
  const picked = $state<Record<string, string>>({});
  const pickedFor = (folder: RootFolder) => picked[folder.id] ?? folder.category ?? '';
  const changedFolders = $derived(
    folders.filter((folder) => pickedFor(folder) !== (folder.category ?? '')),
  );
  holdUnsaved(
    () => changedFolders.length > 0,
    () => t('ConfirmLeaveUnsavedMappings', { count: changedFolders.length }),
  );

  /**
   * The other folders of `folder`'s instance that `category` leads to, as stored or as `written`
   * says a save in progress left them.
   */
  const sharing = (
    folder: RootFolder,
    category: string,
    written: Readonly<Record<string, string | null>> = {},
  ) =>
    folders.filter(
      (other) =>
        other.id !== folder.id &&
        other.instance_id === folder.instance_id &&
        category !== '' &&
        (other.id in written ? written[other.id] : other.category) === category,
    );

  let savingMappings = $state(false);

  /**
   * Writes each pick in turn, the API mapping one folder a request. A pick refused, or a share
   * declined, stays in the bar beside the outcome that names it.
   */
  async function saveMappings() {
    if (savingMappings) return;
    savingMappings = true;
    // What this save wrote, so a later pick is asked about an earlier one.
    const written: Record<string, string | null> = {};
    const failed: { path: string; cause: unknown }[] = [];
    let saved = 0;
    try {
      for (const folder of changedFolders) {
        const category = pickedFor(folder);
        const others = sharing(folder, category, written);
        // Joining a category another folder holds shares its new titles: asked, never assumed.
        if (others.length > 0) {
          const question = t('ConfirmAddToFolderSet', {
            category,
            instance: folder.instance_name,
            folders: others.map((other) => other.path).join(t('ListSeparator')),
            folder: folder.path,
          });
          if (!(await askConfirmation(question, 'AddFolderToCategory', false))) continue;
        }
        try {
          await api.updateRootFolderCategory(folder.id, category || null, others.length > 0);
          written[folder.id] = category || null;
          delete picked[folder.id];
          saved += 1;
        } catch (cause) {
          failed.push({ path: folder.path, cause });
        }
      }
      const [only] = failed;
      if (saved === 0 && only && failed.length === 1) outcome.fail(only.cause);
      else if (failed.length > 0) {
        const message = t('MappingsPartlySaved', { count: saved });
        const named = failed.map(({ path, cause }) =>
          t('FilterTerm', { label: path, value: describeError(cause) }),
        );
        if (saved > 0) outcome.warn(message, named);
        else outcome.fail(message, named);
      } else if (saved > 0) outcome.succeed(t('MappingsSaved', { count: saved }));
      // Mapping a category, or removing one, is what clears the two warnings about categories
      // and instances that reach no folder.
      if (saved > 0) invalidateStatus();
      await bundle.reload();
    } finally {
      savingMappings = false;
      // The bar goes once nothing is pending, and its button with it.
      void handFocus('mappings-save', 'folders-table');
    }
  }

  function discardMappings() {
    for (const id of Object.keys(picked)) delete picked[id];
    void handFocus('folders-table');
  }

  let syncing = $state<string | null>(null);
  /** Stops following, not the sync, which runs on whatever the screen does. */
  const leaving = new AbortController();
  $effect(() => () => leaving.abort());

  /** Offered where a group cannot list its folders yet: never synced, or failing to. */
  const needsSync = (instance: Instance) =>
    instance.enabled && (instance.last_sync_at === null || instance.last_sync_error !== null);

  async function syncNow(instance: Instance) {
    syncing = instance.id;
    try {
      const report = await api.syncInstance(instance.id, { signal: leaving.signal });
      outcome.succeed(
        t('SyncResult', { name: instance.name, media: report.media, folders: report.root_folders }),
      );
      invalidateStatus();
    } catch (cause) {
      if (!leaving.signal.aborted) outcome.fail(cause);
    } finally {
      syncing = null;
    }
    if (leaving.signal.aborted) return;
    await bundle.reload();
    // Synced, its group offers no sync, so the focus goes to the table.
    void handFocus(`sync-${instance.id}`, 'folders-table');
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

  /** Whether to delete the category. One still in use is not offered: the server would refuse. */
  async function confirmRemoval(category: Category): Promise<boolean> {
    const { rule_count, root_folder_count, exception_count, rule_test_count } = category;
    if (rule_count + root_folder_count + exception_count + rule_test_count === 0) {
      return askConfirmation(t('ConfirmDeleteCategory', { name: category.name }), 'Delete');
    }
    const held = t('CategoryHeld', {
      name: category.name,
      rules: rule_count,
      folders: root_folder_count,
      overrides: exception_count,
      tests: rule_test_count,
    });
    await ask(held, []);
    return false;
  }

  /**
   * Declare a destination the instance does not report as a root folder: one
   * root folder per Arr is what an operator keeps there, and the targets
   * beneath it are named here.
   */
  let declaring = $state(false);

  async function declare(event: SubmitEvent, instanceId: string) {
    event.preventDefault();
    const path = (targetPaths[instanceId] ?? '').trim();
    if (!path || declaring) return;
    const pressed = document.activeElement as HTMLElement | null;
    declaring = true;
    try {
      const declared = await act(
        () => api.declareRootFolder(instanceId, path),
        t('DestinationDeclared', { path }),
      );
      if (declared) targetPaths[instanceId] = '';
    } finally {
      declaring = false;
      // Add stays disabled once its path is cleared, so the next path takes the focus.
      void handFocus(pressed, `declare-path-${instanceId}`);
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

  /** A conflict as a banner line, led by its instance unless the sentence names it already. */
  const finding = (conflict: MappingConflict) => ({
    lead:
      conflict.instance_name && !conflict.message.includes(conflict.instance_name)
        ? conflict.instance_name
        : null,
    text: conflict.message,
  });
</script>

<div>
  <div class="page-header">
    <div>
      <h1 class="page-title">{t('CategoriesAndFolders')}</h1>
      <p class="page-subtitle">{t('CategoriesSubtitle')}</p>
    </div>
    {#if !access.readOnly}
      <div class="flex gap-2">
        <button class="btn btn-primary" onclick={() => openDialog(() => (creating = true))}>
          <Plus size={16} />
          {t('NewCategory')}
        </button>
      </div>
    {/if}
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
    items={errors.map(finding)}
  />
  <BannerList
    tone="warning"
    title={t('DiagnosticWarnings', { count: warnings.length })}
    items={warnings.map(finding)}
  />

  {#if bundle.data}
    <FolderSuggestions
      suggestions={bundle.data.suggestions}
      {categories}
      {outcome}
      onChanged={() => bundle.reload()}
    />
  {/if}

  <div class="card">
    <div class="card-header">
      <CardTitle card="FolderMappings">{t('FolderMappings')}</CardTitle>
    </div>
    <TableRegion label={t('FolderMappings')} id="folders-table">
      <table>
        <caption class="visually-hidden">{t('FolderMappings')}</caption>
        <thead>
          <tr>
            <th>{t('Path')}</th>
            <th class="col-secondary">{t('FreeSpace')}</th>
            <th class="col-secondary">{t('State')}</th>
            <th class="w-220">{t('Category')}</th>
            <th class="w-80"><span class="visually-hidden">{t('Actions')}</span></th>
          </tr>
        </thead>
        {#if bundle.loading && groups.length === 0}
          <tbody><TableSkeleton columns={5} /></tbody>
        {:else if groups.length === 0 && !bundle.error}
          <tbody>
            <tr><td colspan="5"><EmptyState>{t('NoRootFolderDiscovered')}</EmptyState></td></tr>
          </tbody>
        {/if}
        {#each groups as group (group.id)}
          {@const instance = group.instance}
          <tbody>
            <tr>
              <th class="folder-group" colspan="5" scope="rowgroup">
                <div class="folder-group-heading flex flex-wrap items-center gap-2">
                  <strong>{group.name}</strong>
                  <span class="text-muted text-sm">
                    {t('WithNote', {
                      text: ARR_NAME[group.type] ?? group.type,
                      note: t(instanceKindKey(group.type)),
                    })}
                  </span>
                  {#if instance}
                    <span class="text-muted text-sm" title={instance.last_sync_at ?? undefined}>
                      {instance.last_sync_at
                        ? t('SyncedWhen', {
                            when: formatRelative(instance.last_sync_at, i18n.language),
                          })
                        : t('NeverSynced')}
                    </span>
                    {#if instance.last_sync_error !== null}
                      <span class="badge badge-danger">{t('StatusFailed')}</span>
                    {/if}
                    {#if !instance.enabled}
                      <span class="badge badge-warning">{t('InstanceDisabled')}</span>
                    {/if}
                    {#if needsSync(instance) && !access.readOnly}
                      <button
                        id="sync-{instance.id}"
                        type="button"
                        class="btn btn-secondary btn-sm"
                        disabled={syncing !== null}
                        aria-label="{t('SyncNow')} – {instance.name}"
                        onclick={() => void syncNow(instance)}
                      >
                        <RefreshCw
                          size={14}
                          class={syncing === instance.id ? 'spin' : ''}
                          aria-hidden="true"
                        />
                        {t('SyncNow')}
                      </button>
                    {/if}
                  {/if}
                </div>
              </th>
            </tr>
            {#each group.folders as folder (folder.id)}
              {@const index = folders.indexOf(folder)}
              {@const shared = folder.category ? sharing(folder, folder.category) : []}
              <tr>
                <td class="mono">
                  <span class="cell-path"><bdi>{folder.path}</bdi></span>
                </td>
                <td class="text-muted col-secondary">
                  {formatBytes(folder.free_space, i18n.language)}
                </td>
                <td class="col-secondary">
                  <!-- A warning with its date, not an error: a NAS that spins down is unreachable
                       every night, and the reader tells a nap from a fault by the date. -->
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
                  {#if access.readOnly}
                    {#if folder.category}
                      <span class="badge badge-value">{folder.category}</span>
                    {:else}
                      <span class="text-muted">{t('Unmapped')}</span>
                    {/if}
                  {:else}
                    <div class="flex flex-wrap items-center gap-2">
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
                      {#if changedFolders.includes(folder)}
                        <span class="badge badge-info">{t('MappingChanged')}</span>
                      {/if}
                    </div>
                  {/if}
                  {#if shared.length > 0}
                    <span class="status-reason">
                      {t('MappingSharedWith', {
                        folders: shared.map((other) => other.path).join(t('ListSeparator')),
                      })}
                    </span>
                  {/if}
                </td>
                <td>
                  <!-- Only what Routarr owns. A folder the instance reports
                       would come back on the next sync, without its category. -->
                  {#if folder.origin === 'declared' && !access.readOnly}
                    <button
                      id={removeId(index)}
                      class="btn btn-danger btn-sm"
                      type="button"
                      aria-label="{t('Remove')} – {folder.path}"
                      title={t('Remove')}
                      onclick={async () => {
                        if (
                          await askConfirmation(
                            t('ConfirmRemoveDestination', { path: folder.path }),
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
                      <Trash2 size={14} aria-hidden="true" />
                    </button>
                  {/if}
                </td>
              </tr>
            {:else}
              <tr>
                <td colspan="5"><p class="folder-group-note text-muted">{t('NoFolderYet')}</p></td>
              </tr>
            {/each}
            <!-- At the foot of the folders it adds to: what an operator is doing here is
                 completing that list. -->
            {#if instance && !access.readOnly}
              <tr>
                <td colspan="5">
                  <details class="declare-fold">
                    <summary class="text-link text-md cursor-pointer">
                      {t('AddDestinationOn', { instance: instance.name })}
                    </summary>
                    <form
                      novalidate
                      class="form-row mt-3"
                      onsubmit={(event) => void declare(event, instance.id)}
                    >
                      <!-- A base width, so on a phone the path takes a line of its own rather
                           than folding its caption beside the button. -->
                      <div class="form-group flex-fill-240">
                        <label class="form-label" for="declare-path-{instance.id}">
                          {t('DeclareDestination')}
                        </label>
                        <input
                          id="declare-path-{instance.id}"
                          class="form-input mono"
                          aria-describedby="declare-path-{instance.id}-help"
                          bind:value={targetPaths[instance.id]}
                          placeholder={t('DeclareDestinationPlaceholder')}
                        />
                      </div>
                      <button
                        class="btn btn-secondary"
                        type="submit"
                        disabled={!(targetPaths[instance.id] ?? '').trim() || declaring}
                      >
                        {declaring ? t('Saving') : t('AddDestination')}
                      </button>
                    </form>
                    <p id="declare-path-{instance.id}-help" class="text-muted text-sm mt-1">
                      {t('DeclareDestinationHelp')}
                    </p>
                  </details>
                </td>
              </tr>
            {/if}
          </tbody>
        {/each}
      </table>
    </TableRegion>

    <!-- Sticky, as the settings save: one write for every pick, counted while it waits. -->
    {#if changedFolders.length > 0}
      <div class="save-bar" role="status">
        <strong>{t('MappingsChanged', { count: changedFolders.length })}</strong>
        <div class="flex gap-2">
          <button type="button" class="btn btn-secondary" onclick={discardMappings}>
            {t('DiscardChanges')}
          </button>
          <button
            id="mappings-save"
            type="button"
            class="btn btn-primary"
            onclick={() => void saveMappings()}
          >
            <Save size={16} aria-hidden="true" />
            {savingMappings ? t('Saving') : t('SaveMappings')}
          </button>
        </div>
      </div>
    {/if}
  </div>

  <div class="card">
    <div class="card-header">
      <CardTitle card="Categories">{t('Categories')}</CardTitle>
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
              <td class="text-muted">
                {#if category.description}{category.description}{:else}<NoValue />{/if}
              </td>
              <td><Count value={category.rule_count} /></td>
              <td><Count value={category.root_folder_count} /></td>
              <td>
                {#if !access.readOnly}
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
                          if (await confirmRemoval(category)) {
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
                {/if}
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
      title={t('RenameCategory')}
      onClose={() => (renaming = null)}
      initialFocus="rootfolders-rename"
    >
      <ErrorBanner message={dialogError} onDismiss={() => (dialogError = null)} />
      <!-- `novalidate`: the browser's bubble speaks the browser's language. Save waits for the
           field instead. -->
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
            aria-describedby="rootfolders-rename-hint"
            bind:value={newName}
            required
          />
          <p id="rootfolders-rename-hint" class="text-muted text-sm mt-1">
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
      title={t('NewCategory')}
      onClose={() => (creating = false)}
      initialFocus="rootfolders-name"
    >
      <ErrorBanner message={dialogError} onDismiss={() => (dialogError = null)} />
      <form novalidate onsubmit={createCategory}>
        <div class="form-group">
          <label class="form-label" for="rootfolders-name">{t('Name')}</label>
          <input
            id="rootfolders-name"
            class="form-input"
            placeholder={t('CategoryNamePlaceholder')}
            aria-describedby="rootfolders-name-hint"
            bind:value={name}
            required
          />
          <p id="rootfolders-name-hint" class="text-muted text-sm mt-1">{t('CategoryNameHint')}</p>
        </div>
        <div class="form-group">
          <label class="form-label" for="rootfolders-description">
            {t('WithNote', { text: t('Description'), note: t('Optional') })}
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
