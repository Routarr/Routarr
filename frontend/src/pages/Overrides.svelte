<script lang="ts">
  import { Lock, Plus, Search, Trash2 } from '../lib/icons';
  import { api } from '../api/client';
  import type { Category, MediaListItem } from '../api/types';
  import { createAsync, describeError } from '../lib/async.svelte';
  import { createOutcome } from '../lib/outcome.svelte';
  import { i18n, t } from '../lib/i18n.svelte';
  import { formatTimestamp } from '../api/format';
  import EmptyState from '../components/EmptyState.svelte';
  import ErrorBanner from '../components/ErrorBanner.svelte';
  import Loading from '../components/Loading.svelte';
  import Modal from '../components/Modal.svelte';
  import OutcomeBanner from '../components/OutcomeBanner.svelte';
  import { askConfirmation } from '../lib/confirm.svelte';
  import TableRegion from '../components/TableRegion.svelte';
  import SearchField from '../components/SearchField.svelte';

  const bundle = createAsync(async (signal) => {
    const [overrides, categories] = await Promise.all([
      api.getOverrides(signal),
      api.getCategories(signal),
    ]);
    return { overrides, categories };
  });

  const outcome = createOutcome();
  let creating = $state(false);

  const overrides = $derived(bundle.data?.overrides ?? []);
  const categories = $derived<Category[]>(bundle.data?.categories ?? []);

  async function remove(id: string, title: string) {
    if (!(await askConfirmation(t('ConfirmDeleteOverride', { title }), 'Delete'))) return;
    try {
      await api.deleteOverride(id);
      outcome.succeed(t('OverrideRemoved'));
      await bundle.reload();
    } catch (err) {
      outcome.fail(err);
    }
  }

  // ---------------------------------------------------------------- creation
  let search = $state('');
  let results = $state<MediaListItem[]>([]);
  let selected = $state<MediaListItem | null>(null);
  /**
   * Derived, not captured. Reading `categories[0]` when the button is clicked
   * races the request that fills them: open the modal before the list lands and
   * the override is created against an empty category, which the backend then
   * refuses for a field the user never touched.
   */
  let chosenCategory = $state<string | null>(null);
  const category = $derived(chosenCategory ?? categories[0]?.name ?? '');
  let reason = $state('');
  let locked = $state(true);

  function openCreate() {
    search = '';
    results = [];
    selected = null;
    chosenCategory = null;
    reason = '';
    locked = true;
    dialogError = null;
    creating = true;
  }

  // Inside the dialog, for the same reason `Instances` keeps its own: the page
  // banner is under the modal, dimmed and inert.
  let dialogError = $state<string | null>(null);

  async function find(event: SubmitEvent) {
    event.preventDefault();
    dialogError = null;
    try {
      const page = await api.getMedia({ search, per_page: 15 });
      results = page.data;
    } catch (err) {
      dialogError = describeError(err);
    }
  }

  async function save() {
    if (!selected) return;
    try {
      await api.createOverride({
        media_id: selected.id,
        target_category: category,
        reason: reason || null,
        locked,
      });
      creating = false;
      outcome.succeed(t('OverrideCreated', { title: selected.title, category }));
      await bundle.reload();
    } catch (err) {
      dialogError = describeError(err);
    }
  }
</script>

<div>
  <div class="page-header">
    <div>
      <h1 class="page-title">{t('Overrides')}</h1>
      <p class="page-subtitle">{t('OverridesSubtitle')}</p>
    </div>
    <button class="btn btn-primary" onclick={openCreate}>
      <Plus size={16} />
      {t('NewOverride')}
    </button>
  </div>

  <ErrorBanner
    message={bundle.error}
    onDismiss={() => (bundle.error = null)}
    onRetry={() => void bundle.reload()}
  />
  <OutcomeBanner {outcome} />

  <div class="card">
    <TableRegion label={t('Overrides')}>
      <table>
        <caption class="visually-hidden">{t('Overrides')}</caption>
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
            <tr><td colspan="6"><Loading /></td></tr>
          {:else if overrides.length === 0}
            <tr><td colspan="6"><EmptyState>{t('NoOverrides')}</EmptyState></td></tr>
          {:else}
            {#each overrides as override (override.id)}
              <tr>
                <td>
                  <strong>{override.media_title}</strong>
                  <span class="badge badge-value muted">{override.media_type}</span>
                  {#if override.locked}
                    <span class="badge badge-warning ms-1">
                      <Lock size={11} />
                      {t('Locked')}
                    </span>
                  {/if}
                </td>
                <td>{override.instance_name}</td>
                <td><span class="badge badge-value">{override.target_category}</span></td>
                <td class="text-muted">{override.reason ?? t('None')}</td>
                <td class="text-muted mono text-sm">
                  {formatTimestamp(override.created_at, i18n.language)}
                </td>
                <td>
                  <button
                    class="btn btn-danger btn-sm"
                    aria-label="{t('Delete')} – {override.media_title}"
                    title={t('Delete')}
                    onclick={() => void remove(override.id, override.media_title)}
                  >
                    <Trash2 size={14} />
                  </button>
                </td>
              </tr>
            {/each}
          {/if}
        </tbody>
      </table>
    </TableRegion>
  </div>

  {#if creating}
    <Modal label={t('PinMediaTitle')} onClose={() => (creating = false)} maxWidth={640}>
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
          bind:value={search}
          placeholder={t('SearchLibrary')}
          label={t('SearchLibrary')}
        />
        <button class="btn btn-secondary" type="submit">
          <Search size={16} />
          {t('Search')}
        </button>
      </form>

      {#if results.length > 0}
        <TableRegion label={t('Search')} class="mt-4 scroll-y-220">
          <table>
            <caption class="visually-hidden">{t('Search')}</caption>
            <tbody>
              {#each results as media (media.id)}
                <tr class:row-selected={selected?.id === media.id}>
                  <!-- A button, not a clickable row: a row has no tab stop, no
                       role and no key, and Svelte warns about that on a div but
                       not on a tr. Named `action — subject`, the convention the
                       accessibility sweep enforces on row actions. -->
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
                    {media.current_root_folder ?? t('None')}
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
            {t('Reason')} ({t('Optional')})
          </label>
          <input
            id="overrides-reason"
            class="form-input"
            placeholder={t('ReasonPlaceholder')}
            bind:value={reason}
          />
        </div>

        <label class="flex items-center gap-2 text-base">
          <input type="checkbox" bind:checked={locked} />
          {t('LockClassification')}
        </label>
      {/if}

      <div class="flex justify-between mt-4">
        <button class="btn btn-secondary" onclick={() => (creating = false)}>{t('Cancel')}</button>
        <button class="btn btn-primary" onclick={save} disabled={!selected}>
          {t('CreateOverride')}
        </button>
      </div>
    </Modal>
  {/if}
</div>
