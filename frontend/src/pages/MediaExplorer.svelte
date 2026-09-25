<script lang="ts">
  import { Film, HelpCircle, Lock, Tv } from '../lib/icons';
  import { api } from '../api/client';
  import type { Explanation, MediaListItem } from '../api/types';
  import { createAsync } from '../lib/async.svelte';
  import { createOutcome } from '../lib/outcome.svelte';
  import { t } from '../lib/i18n.svelte';
  import EmptyState from '../components/EmptyState.svelte';
  import ErrorBanner from '../components/ErrorBanner.svelte';
  import OutcomeBanner from '../components/OutcomeBanner.svelte';
  import ExplanationModal from '../components/ExplanationModal.svelte';
  import TableSkeleton from '../components/TableSkeleton.svelte';
  import TableRegion from '../components/TableRegion.svelte';
  import SearchField from '../components/SearchField.svelte';

  let search = $state('');
  let pending = $state('');
  let mediaType = $state('');
  let unmatched = $state(false);
  let page = $state(1);
  let explaining = $state<Explanation | null>(null);

  const library = createAsync(
    (signal) =>
      api.getMedia(
        {
          search: search || undefined,
          media_type: mediaType || undefined,
          unmatched: unmatched || undefined,
          page,
          per_page: 50,
        },
        signal,
      ),
    () => [search, mediaType, unmatched, page],
  );
  const outcome = createOutcome();

  const items = $derived(library.data?.data ?? []);
  const pagination = $derived(library.data?.pagination);

  async function explain(media: MediaListItem) {
    try {
      explaining = await api.explainMedia(media.id);
      outcome.clear();
    } catch (err) {
      outcome.fail(err);
    }
  }
</script>

<div>
  <div class="page-header">
    <div>
      <h1 class="page-title">{t('MediaExplorer')}</h1>
      <p class="page-subtitle">{t('MediaExplorerSubtitle')}</p>
    </div>
  </div>

  <ErrorBanner
    message={library.error}
    onDismiss={() => (library.error = null)}
    onRetry={() => void library.reload()}
  />
  <OutcomeBanner {outcome} />

  <div class="toolbar">
    <form
      novalidate
      class="flex items-center gap-2 flex-1 flex-wrap"
      onsubmit={(event) => {
        event.preventDefault();
        page = 1;
        search = pending;
      }}
    >
      <SearchField
        bind:value={pending}
        placeholder={t('SearchByTitle')}
        label={t('SearchLibrary')}
      />
      <select
        class="form-select"
        aria-label={t('FilterByType')}
        value={mediaType}
        onchange={(event) => {
          page = 1;
          mediaType = event.currentTarget.value;
        }}
      >
        <option value="">{t('AllTypes')}</option>
        <option value="movie">{t('Movies')}</option>
        <option value="series">{t('Series')}</option>
      </select>
      <label class="flex items-center gap-2 text-md">
        <input
          type="checkbox"
          checked={unmatched}
          onchange={(event) => {
            page = 1;
            unmatched = event.currentTarget.checked;
          }}
        />
        {t('UnclassifiedOnly')}
      </label>
      <button type="submit" class="btn btn-secondary">{t('Search')}</button>
      <!-- Offered only while a filter is active: undoing three of them one by
           one is the friction this removes, and a button that does nothing is
           noise. -->
      {#if search || pending || mediaType || unmatched}
        <button
          type="button"
          class="btn btn-ghost"
          onclick={() => {
            pending = '';
            search = '';
            mediaType = '';
            unmatched = false;
            page = 1;
          }}>{t('ClearFilters')}</button
        >
      {/if}
    </form>
  </div>

  <div class="card">
    <TableRegion label={t('MediaExplorer')}>
      <table>
        <caption class="visually-hidden">{t('MediaExplorer')}</caption>
        <thead>
          <tr>
            <th>{t('Title')}</th>
            <th>{t('Instance')}</th>
            <th>{t('CurrentRootFolder')}</th>
            <th>{t('ProposedCategory')}</th>
            <th>{t('Metadata')}</th>
            <th class="w-120">{t('Actions')}</th>
          </tr>
        </thead>
        <tbody>
          {#if library.loading && items.length === 0}
            <TableSkeleton columns={6} />
          {:else if items.length === 0}
            <tr><td colspan="6"><EmptyState>{t('NoMediaMatches')}</EmptyState></td></tr>
          {:else}
            {#each items as media (media.id)}
              <tr>
                <td>
                  <div class="flex items-center gap-2">
                    {#if media.media_type === 'movie'}
                      <Film size={16} class="text-radarr" />
                    {:else}
                      <Tv size={16} class="text-sonarr" />
                    {/if}
                    <!-- One line, the whole title on hover. A long title wrapped
                         and took its row to twice the height of its neighbours,
                         and the year drifted off on its own. -->
                    <strong class="cell-title" title={media.title}>{media.title}</strong>
                    {#if media.year}<span class="text-muted">({media.year})</span>{/if}
                  </div>
                </td>
                <td>{media.instance_name}</td>
                <td class="mono text-sm">
                  <span class="cell-path" title={media.current_root_folder ?? undefined}>
                    {media.current_root_folder ?? t('None')}
                  </span>
                </td>
                <td>
                  {#if media.override_category}
                    <span class="badge badge-value" title={t('ManualOverride')}>
                      <Lock size={11} aria-hidden="true" />
                      {media.override_category}
                    </span>
                  {:else if media.computed_category}
                    <span class="badge badge-value">{media.computed_category}</span>
                  {:else}
                    <!-- A pill either way. One value as a pill and the next as
                         grey prose read as two kinds of data in one column. -->
                    <span class="badge badge-value muted">{t('NotEvaluated')}</span>
                  {/if}
                </td>
                <td>
                  <!-- Having metadata is not an outcome, so it carries no status
                       colour; missing it stops genre and keyword rules from
                       matching, which is a warning. -->
                  <span class="badge {media.has_metadata ? 'badge-value muted' : 'badge-warning'}">
                    {t(media.has_metadata ? 'MetadataCached' : 'MetadataMissing')}
                  </span>
                </td>
                <td>
                  <!-- The action is the same on every line, so it does not need
                       spelling out once per row. -->
                  <button
                    class="btn btn-ghost btn-sm"
                    onclick={() => void explain(media)}
                    aria-label="{t('WhyQuestion')} {media.title}"
                    title={t('WhyQuestion')}
                  >
                    <HelpCircle size={15} aria-hidden="true" />
                  </button>
                </td>
              </tr>
            {/each}
          {/if}
        </tbody>
      </table>
    </TableRegion>

    {#if pagination && pagination.total_pages > 1}
      <div class="flex items-center justify-between mt-4">
        <span class="text-muted text-md">
          {t('PageOf', { page: pagination.page, total: pagination.total_pages })} · {t(
            'ItemCount',
            { count: pagination.total },
          )}
        </span>
        <div class="flex gap-2">
          <button class="btn btn-secondary btn-sm" disabled={page <= 1} onclick={() => (page -= 1)}>
            {t('Previous')}
          </button>
          <button
            class="btn btn-secondary btn-sm"
            disabled={page >= pagination.total_pages}
            onclick={() => (page += 1)}
          >
            {t('Next')}
          </button>
        </div>
      </div>
    {/if}
  </div>

  {#if explaining}
    <ExplanationModal data={explaining} onClose={() => (explaining = null)} />
  {/if}
</div>
