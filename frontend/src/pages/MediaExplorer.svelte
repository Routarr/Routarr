<script lang="ts">
  import { Film, HelpCircle, Lock, RefreshCw, Tv } from '../lib/icons';
  import { api } from '../api/client';
  import { mediaTypeKey } from '../api/format';
  import type { Explanation, MediaListItem } from '../api/types';
  import { createAsync } from '../lib/async.svelte';
  import { createOutcome } from '../lib/outcome.svelte';
  import { t } from '../lib/i18n.svelte';
  import NoValue from '../components/NoValue.svelte';
  import EmptyState from '../components/EmptyState.svelte';
  import ErrorBanner from '../components/ErrorBanner.svelte';
  import OutcomeBanner from '../components/OutcomeBanner.svelte';
  import ExplanationModal from '../components/ExplanationModal.svelte';
  import TableSkeleton from '../components/TableSkeleton.svelte';
  import TableRegion from '../components/TableRegion.svelte';
  import Pager from '../components/Pager.svelte';
  import SearchField from '../components/SearchField.svelte';

  let search = $state('');
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

  // Only the last title asked about answers: two quick clicks would otherwise
  // show whichever explanation lands last, under the other title's row.
  let asking: AbortController | null = null;
  /** The title whose explanation is on its way, which its row says. */
  let asked = $state<string | null>(null);

  async function explain(media: MediaListItem) {
    asking?.abort();
    const mine = (asking = new AbortController());
    asked = media.id;
    try {
      const answer = await api.explainMedia(media.id, mine.signal);
      if (mine.signal.aborted) return;
      explaining = answer;
      outcome.clear();
    } catch (err) {
      if (!mine.signal.aborted) outcome.fail(err);
    } finally {
      if (!mine.signal.aborted) asked = null;
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
    <SearchField
      bind:value={search}
      placeholder={t('SearchByTitle')}
      label={t('SearchLibrary')}
      oninput={() => (page = 1)}
      debounce={200}
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
    <!-- Offered only while a filter is active: undoing three of them one by
         one is the friction this removes, and a button that does nothing is
         noise. -->
    {#if search || mediaType || unmatched}
      <button
        type="button"
        class="btn btn-ghost"
        onclick={() => {
          search = '';
          mediaType = '';
          unmatched = false;
          page = 1;
        }}>{t('ClearFilters')}</button
      >
    {/if}
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
            <th class="w-120"><span class="visually-hidden">{t('Actions')}</span></th>
          </tr>
        </thead>
        <tbody>
          {#if library.loading && items.length === 0}
            <TableSkeleton columns={6} />
          {:else if items.length === 0 && !library.error}
            <tr><td colspan="6"><EmptyState>{t('NoMediaMatches')}</EmptyState></td></tr>
          {:else}
            {#each items as media (media.id)}
              <tr>
                <td>
                  <div class="flex items-center gap-2">
                    <!-- The icon for the eye, the word for a screen reader. -->
                    {#if media.media_type === 'movie'}
                      <Film size={16} class="text-radarr" />
                    {:else}
                      <Tv size={16} class="text-sonarr" />
                    {/if}
                    <span class="visually-hidden">{t(mediaTypeKey(media.media_type))}</span>
                    <strong class="cell-title">{media.title}</strong>
                    {#if media.year}<span class="text-muted">({media.year})</span>{/if}
                  </div>
                </td>
                <td>{media.instance_name}</td>
                <td class="mono text-sm">
                  <span class="cell-path">
                    {#if media.current_root_folder}
                      <bdi>{media.current_root_folder}</bdi>
                    {:else}
                      <NoValue />
                    {/if}
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
                         grey prose would read as two kinds of data in one
                         column. -->
                    <span class="badge badge-value muted">{t('NotEvaluated')}</span>
                  {/if}
                </td>
                <td>
                  <!-- Having metadata is not an outcome, so it carries no status
                       colour. Missing it stops genre and keyword rules from
                       matching, which is a warning. -->
                  <span class="badge {media.has_metadata ? 'badge-value muted' : 'badge-warning'}">
                    {t(media.has_metadata ? 'MetadataCached' : 'MetadataMissing')}
                  </span>
                </td>
                <td>
                  <!-- The action is the same on every line, so it does not need
                       spelling out once per row. -->
                  <button
                    class="btn btn-secondary btn-sm"
                    onclick={() => void explain(media)}
                    aria-label="{t('WhyQuestion')} – {media.title}"
                    aria-busy={asked === media.id}
                    title={t('WhyQuestion')}
                  >
                    {#if asked === media.id}
                      <RefreshCw size={15} class="spin" aria-hidden="true" />
                    {:else}
                      <HelpCircle size={15} aria-hidden="true" />
                    {/if}
                  </button>
                </td>
              </tr>
            {/each}
          {/if}
        </tbody>
      </table>
    </TableRegion>

    <Pager {pagination} bind:page countKey="ItemCount" />
  </div>

  {#if explaining}
    <ExplanationModal data={explaining} onClose={() => (explaining = null)} />
  {/if}
</div>
