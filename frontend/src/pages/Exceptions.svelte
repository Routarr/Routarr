<script lang="ts">
  import { Plus, Search, Trash2 } from '../lib/icons';
  import { api } from '../api/client';
  import type { Category, MediaListItem } from '../api/types';
  import { createAsync, describeError } from '../lib/async.svelte';
  import { createOutcome } from '../lib/outcome.svelte';
  import { i18n, t } from '../lib/i18n.svelte';
  import { formatTimestamp, mediaTypeKey } from '../api/format';
  import NoValue from '../components/NoValue.svelte';
  import EmptyState from '../components/EmptyState.svelte';
  import ErrorBanner from '../components/ErrorBanner.svelte';
  import Modal from '../components/Modal.svelte';
  import OutcomeBanner from '../components/OutcomeBanner.svelte';
  import { askConfirmation } from '../lib/confirm.svelte';
  import { handFocus } from '../lib/focus';
  import { invalidateStatus } from '../lib/status.svelte';
  import { access } from '../lib/access.svelte';
  import TableSkeleton from '../components/TableSkeleton.svelte';
  import TableRegion from '../components/TableRegion.svelte';
  import SearchField from '../components/SearchField.svelte';
  import Pager from '../components/Pager.svelte';

  let page = $state(1);
  const bundle = createAsync(
    async (signal) => {
      const [overrides, categories] = await Promise.all([
        api.getExceptions({ page, per_page: 50 }, signal),
        api.getCategories(signal),
      ]);
      return { overrides, categories };
    },
    () => [page],
  );

  const outcome = createOutcome();
  let creating = $state(false);

  const overrides = $derived(bundle.data?.overrides.data ?? []);
  const categories = $derived<Category[]>(bundle.data?.categories ?? []);

  // A removal takes the pressed Delete with it, so the focus goes to the nearest row left.
  const deleteId = (index: number) => `overrides-delete-${index}`;

  async function remove(id: string, title: string, index: number) {
    if (!(await askConfirmation(t('ConfirmDeleteOverride', { title }), 'Delete'))) return;
    try {
      await api.deleteException(id);
      outcome.succeed(t('OverrideRemoved'));
      invalidateStatus();
      await bundle.reload();
      // The last exception of the last page leaves the page before it to show.
      if (overrides.length === 0 && page > 1) page -= 1;
      void handFocus(deleteId(index), deleteId(index - 1), 'exceptions-table');
    } catch (err) {
      outcome.fail(err);
    }
  }

  let search = $state('');
  let results = $state<MediaListItem[]>([]);
  /** How many titles matched: the search answers fifteen at most. */
  let matched = $state(0);
  let selected = $state<MediaListItem | null>(null);
  /**
   * Derived, since the categories may land after the modal opens, and an empty category is
   * refused for a field the user never touched.
   */
  let chosenCategory = $state<string | null>(null);
  const category = $derived(chosenCategory ?? categories[0]?.name ?? '');
  let reason = $state('');

  function openCreate() {
    search = '';
    results = [];
    matched = 0;
    searched = false;
    selected = null;
    chosenCategory = null;
    reason = '';
    dialogError = null;
    creating = true;
  }

  // Inside the dialog: the page banner lies under the modal, dimmed and inert.
  let dialogError = $state<string | null>(null);
  // An empty answer says so, or it looks like a click that did nothing.
  let searched = $state(false);
  /** Whether the library holds no title, which a sync fills. */
  let libraryEmpty = $state(false);

  // Only the last search answers, and one the screen closed on is abandoned.
  let finding: AbortController | null = null;
  $effect(() => () => finding?.abort());

  async function find(event: SubmitEvent) {
    event.preventDefault();
    dialogError = null;
    finding?.abort();
    const mine = (finding = new AbortController());
    try {
      const page = await api.getLibrary({ search, per_page: 15 }, mine.signal);
      // A blank search reads the whole library, and one that found titles shows it holds some.
      const held =
        page.pagination.total > 0 || !search.trim()
          ? page.pagination.total
          : (await api.getLibrary({ per_page: 1 }, mine.signal)).pagination.total;
      results = page.data;
      matched = page.pagination.total;
      libraryEmpty = held === 0;
      searched = true;
    } catch (err) {
      if (!mine.signal.aborted) dialogError = describeError(err);
    }
  }

  let saving = $state(false);

  async function save() {
    if (!selected || saving) return;
    const pressed = document.activeElement as HTMLElement | null;
    saving = true;
    try {
      await api.createException({
        media_id: selected.id,
        target_category: category,
        reason: reason || null,
      });
      creating = false;
      outcome.succeed(t('OverrideCreated', { title: selected.title, category }));
      invalidateStatus();
      await bundle.reload();
    } catch (err) {
      dialogError = describeError(err);
      void handFocus(pressed);
    } finally {
      saving = false;
    }
  }
</script>

<div>
  <div class="page-header">
    <div>
      <h1 class="page-title">{t('Exceptions')}</h1>
      <p class="page-subtitle">{t('ExceptionsSubtitle')}</p>
    </div>
    {#if !access.readOnly}
      <div class="flex gap-2">
        <button class="btn btn-primary" onclick={openCreate}>
          <Plus size={16} />
          {t('NewOverride')}
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

  <div class="card">
    <TableRegion label={t('Exceptions')} id="exceptions-table">
      <table>
        <caption class="visually-hidden">{t('Exceptions')}</caption>
        <thead>
          <tr>
            <th>{t('Media')}</th>
            <th>{t('Instance')}</th>
            <th>{t('ForcedCategory')}</th>
            <th>{t('Reason')}</th>
            <th>{t('Created')}</th>
            <th class="w-80"><span class="visually-hidden">{t('Actions')}</span></th>
          </tr>
        </thead>
        <tbody>
          {#if bundle.loading && overrides.length === 0}
            <TableSkeleton columns={6} />
          {:else if overrides.length === 0 && !bundle.error}
            <tr><td colspan="6"><EmptyState>{t('NoOverrides')}</EmptyState></td></tr>
          {:else}
            {#each overrides as override, index (override.id)}
              <tr>
                <td>
                  <strong>{override.media_title}</strong>
                  <span class="badge badge-value muted">{t(mediaTypeKey(override.media_type))}</span
                  >
                </td>
                <td>{override.instance_name}</td>
                <td><span class="badge badge-value">{override.target_category}</span></td>
                <td class="text-muted">
                  {#if override.reason}{override.reason}{:else}<NoValue />{/if}
                </td>
                <td class="cell-timestamp">
                  <span>{formatTimestamp(override.created_at, i18n.language)}</span>
                  {#if override.subject}
                    <span class="text-xs text-muted" title={t('PerformedBy')}
                      ><span class="visually-hidden">{t('PerformedBy')}</span
                      >{override.subject}</span
                    >
                  {/if}
                </td>
                <td>
                  {#if !access.readOnly}
                    <button
                      class="btn btn-danger btn-sm"
                      aria-label="{t('Delete')} – {override.media_title}"
                      title={t('Delete')}
                      id={deleteId(index)}
                      onclick={() => void remove(override.id, override.media_title, index)}
                    >
                      <Trash2 size={14} />
                    </button>
                  {/if}
                </td>
              </tr>
            {/each}
          {/if}
        </tbody>
      </table>
    </TableRegion>
    <Pager pagination={bundle.data?.overrides.pagination} bind:page countKey="OverrideCount" />
  </div>

  {#if creating}
    <Modal
      label={t('PinMediaTitle')}
      onClose={() => (creating = false)}
      maxWidth={640}
      initialFocus="overrides-search"
    >
      <div class="modal-header">
        <h2 class="modal-title">{t('PinMediaTitle')}</h2>
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

      <form novalidate class="flex gap-2" onsubmit={find}>
        <SearchField
          id="overrides-search"
          bind:value={search}
          placeholder={t('SearchLibrary')}
          label={t('SearchLibrary')}
        />
        <button class="btn btn-secondary" type="submit">
          <Search size={16} />
          {t('Search')}
        </button>
      </form>

      <!-- Mounted before the first search, so what each one finds is announced:
           a table drawn under the field says nothing to a screen reader. -->
      <div role="status" class="mt-4">
        {#if searched && results.length === 0}
          <p class="text-muted text-sm">
            {t(libraryEmpty ? 'NoMediaMatches' : 'NoMediaMatchesFilters')}
          </p>
        {:else if matched > results.length}
          <p class="text-muted text-sm">
            {t('SearchShowingFirst', { shown: results.length, total: matched })}
          </p>
        {:else if searched}
          <p class="visually-hidden">{t('CommandPaletteResults', { count: results.length })}</p>
        {/if}
      </div>

      {#if results.length > 0}
        <TableRegion label={t('Search')} class="mt-2 scroll-y-220">
          <table>
            <caption class="visually-hidden">{t('Search')}</caption>
            <thead>
              <tr>
                <th>{t('Title')}</th>
                <th>{t('Instance')}</th>
                <th>{t('CurrentRootFolder')}</th>
              </tr>
            </thead>
            <tbody>
              {#each results as media (media.id)}
                <tr class:row-selected={selected?.id === media.id}>
                  <!-- A button: a row has no tab stop, no role and no key. Named
                       `action – subject`, as the accessibility sweep wants. -->
                  <td>
                    <button
                      type="button"
                      class="row-pick"
                      aria-label="{t('SelectItem')} – {media.title}"
                      aria-pressed={selected?.id === media.id}
                      onclick={() => (selected = media)}
                    >
                      <strong>{media.title}</strong>
                      <span class="text-muted">{media.year ?? ''}</span>
                    </button>
                  </td>
                  <td class="text-muted">{media.instance_name}</td>
                  <td class="mono text-sm">
                    {#if media.current_root_folder}{media.current_root_folder}{:else}<NoValue
                      />{/if}
                  </td>
                </tr>
              {/each}
            </tbody>
          </table>
        </TableRegion>
      {/if}

      {#if selected}
        <div class="form-group mt-4">
          <label class="form-label" for="overrides-target-category">
            {t('ForceCategoryFor', { title: selected.title })}
          </label>
          <select
            id="overrides-target-category"
            class="form-select"
            value={category}
            onchange={(event) => (chosenCategory = event.currentTarget.value)}
          >
            {#each categories as option (option.id)}
              <option value={option.name}>{option.name}</option>
            {/each}
          </select>
        </div>

        <div class="form-group">
          <label class="form-label" for="overrides-reason">
            {t('WithNote', { text: t('Reason'), note: t('Optional') })}
          </label>
          <input
            id="overrides-reason"
            class="form-input"
            placeholder={t('ReasonPlaceholder')}
            bind:value={reason}
          />
        </div>
      {/if}

      <div class="dialog-actions">
        <button class="btn btn-secondary" onclick={() => (creating = false)}>{t('Cancel')}</button>
        <button class="btn btn-primary" onclick={save} disabled={!selected || saving}>
          {saving ? t('Saving') : t('CreateOverride')}
        </button>
      </div>
    </Modal>
  {/if}
</div>
