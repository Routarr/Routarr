<script lang="ts">
  import { ArrowRight, Ban, Check, Film, Lock, Tv } from '../lib/icons';
  import { api } from '../api/client';
  import { formatCount, mediaTypeKey } from '../api/format';
  import { createAsync } from '../lib/async.svelte';
  import { createExplainer } from '../lib/explain.svelte';
  import { cut, i18n, t } from '../lib/i18n.svelte';
  import NoValue from '../components/NoValue.svelte';
  import EmptyState from '../components/EmptyState.svelte';
  import ErrorBanner from '../components/ErrorBanner.svelte';
  import ExplanationModal from '../components/ExplanationModal.svelte';
  import WhyButton from '../components/WhyButton.svelte';
  import TableSkeleton from '../components/TableSkeleton.svelte';
  import TableRegion from '../components/TableRegion.svelte';
  import Pager from '../components/Pager.svelte';
  import SearchField from '../components/SearchField.svelte';
  import { keepInAddress } from '../lib/address.svelte';

  /** `move` and `skip` are the outcomes the server filters on, `unclassified` no rule matched. */
  const OUTCOMES = ['move', 'skip', 'unclassified'] as const;
  let search = $state('');
  let mediaType = $state('');
  let outcome = $state<'' | (typeof OUTCOMES)[number]>('');
  const filtered = $derived(Boolean(search || mediaType || outcome));
  let page = $state(1);
  keepInAddress(
    () => ({ search, type: mediaType, outcome }),
    (read) => {
      search = read('search');
      mediaType = read('type', ['movie', 'series']);
      outcome = read('outcome', OUTCOMES) as typeof outcome;
      page = 1;
    },
  );

  const library = createAsync(
    (signal) =>
      api.getLibrary(
        {
          search: search || undefined,
          media_type: mediaType || undefined,
          action: outcome === 'move' || outcome === 'skip' ? outcome : undefined,
          unmatched: outcome === 'unclassified' || undefined,
          page,
          per_page: 50,
        },
        signal,
      ),
    () => [search, mediaType, outcome, page],
  );
  /** Of the whole library, whatever the filters: no genre or keyword rule can match these. */
  const bare = createAsync((signal) => api.countLibrary({ without_metadata: true }, signal));
  const explainer = createExplainer();

  function reload() {
    void library.reload();
    void bare.reload();
  }

  const items = $derived(library.data?.data ?? []);
  const pagination = $derived(library.data?.pagination);

  /**
   * A value and its note as the language sets one, the note muted with its brackets: a title's
   * year, and the word that says a category is forced.
   */
  const withNote = $derived(cut('WithNote', ['text', 'note']));
  const movesTo = $derived(cut('LibraryMovesTo', ['folder']));
</script>

<div>
  <div class="page-header">
    <div>
      <h1 class="page-title">{t('Library')}</h1>
      <p class="page-subtitle">{t('LibrarySubtitle')}</p>
    </div>
  </div>

  <ErrorBanner
    message={library.error ?? bare.error}
    onDismiss={() => {
      library.error = null;
      bare.error = null;
    }}
    onRetry={reload}
  />
  <ErrorBanner message={explainer.failure} onDismiss={explainer.dismiss} />

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
    <select
      class="form-select"
      aria-label={t('FilterByOutcome')}
      value={outcome}
      onchange={(event) => {
        page = 1;
        outcome = event.currentTarget.value as typeof outcome;
      }}
    >
      <option value="">{t('AllOutcomes')}</option>
      <option value="move">{t('OutcomeWillMove')}</option>
      <option value="skip">{t('OutcomeNoFolder')}</option>
      <option value="unclassified">{t('OutcomeUnclassified')}</option>
    </select>
    <!-- Offered only while a filter is active: a button that does nothing is noise. -->
    {#if filtered}
      <button
        type="button"
        class="btn btn-ghost"
        onclick={() => {
          search = '';
          mediaType = '';
          outcome = '';
          page = 1;
        }}>{t('ClearFilters')}</button
      >
    {/if}
    {#if (bare.data ?? 0) > 0}
      <span class="badge badge-warning">
        {t('LibraryWithoutMetadata', { count: formatCount(bare.data ?? 0, i18n.language) })}
      </span>
    {/if}
  </div>

  <div class="card">
    <TableRegion label={t('Library')}>
      <table>
        <caption class="visually-hidden">{t('Library')}</caption>
        <!-- What the screen answers first: on a phone the secondary columns go. -->
        <thead>
          <tr>
            <th>{t('Title')}</th>
            <th>{t('LibraryRouting')}</th>
            <th class="col-secondary">{t('CurrentRootFolder')}</th>
            <th class="col-secondary">{t('Instance')}</th>
          </tr>
        </thead>
        <tbody>
          {#if library.loading && items.length === 0}
            <TableSkeleton columns={4} />
          {:else if items.length === 0 && !library.error}
            <tr>
              <td colspan="4">
                <EmptyState>{t(filtered ? 'NoMediaMatchesFilters' : 'NoMediaMatches')}</EmptyState>
              </td>
            </tr>
          {:else}
            {#each items as media (media.id)}
              <tr>
                <td>
                  <div class="flex flex-wrap items-center gap-2">
                    <!-- The icon for the eye, the word for a screen reader. -->
                    {#if media.media_type === 'movie'}
                      <Film size={16} class="text-radarr" />
                    {:else}
                      <Tv size={16} class="text-sonarr" />
                    {/if}
                    <span class="visually-hidden">{t(mediaTypeKey(media.media_type))}</span>
                    <!-- One block, so the year follows the title's last word
                         rather than the edge of the room a title keeps. -->
                    <span class="cell-title">
                      {#if media.year}
                        {@const [before, between, after] = withNote}
                        {before}<strong>{media.title}</strong><span class="text-muted"
                          >{between}{media.year}{after}</span
                        >
                      {:else}
                        <strong>{media.title}</strong>
                      {/if}
                    </span>
                    <WhyButton {explainer} mediaId={media.id} title={media.title} />
                    {#if !media.has_metadata}
                      <!-- Missing it stops genre and keyword rules matching. -->
                      <span class="badge badge-warning">{t('NoMetadata')}</span>
                    {/if}
                  </div>
                </td>
                <td>
                  {#if media.override_category}
                    {@const [before, between, after] = withNote}
                    {before}<span class="badge badge-value"
                      ><Lock size={11} aria-hidden="true" />{media.override_category}</span
                    ><span class="text-muted text-sm"
                      >{between}{t('LibraryExceptionNote')}{after}</span
                    >
                  {:else if media.computed_category}
                    <span class="badge badge-value">{media.computed_category}</span>
                  {:else}
                    <!-- A pill either way: a pill beside grey prose reads as two kinds
                         of data in one column. -->
                    <span class="badge badge-value muted">{t('NotEvaluated')}</span>
                  {/if}
                  {#if media.action === 'move' && media.target_root_folder}
                    {@const [before, after] = movesTo}
                    <div class="routing-outcome flex items-center gap-1 mt-1 text-xs text-muted">
                      <ArrowRight size={14} class="text-warning dir-aware" aria-hidden="true" />
                      <span>{before}<bdi class="mono">{media.target_root_folder}</bdi>{after}</span>
                    </div>
                  {:else if media.action === 'none'}
                    <div class="routing-outcome flex items-center gap-1 mt-1 text-xs text-muted">
                      <Check size={14} class="text-success" aria-hidden="true" />
                      <span>{t('LibraryStays')}</span>
                    </div>
                  {:else if media.action === 'skip'}
                    <div class="routing-outcome flex items-center gap-1 mt-1 text-xs text-muted">
                      <Ban size={14} class="text-danger" aria-hidden="true" />
                      <span>{t('LibraryNoFolder')}</span>
                    </div>
                  {/if}
                </td>
                <td class="mono text-sm col-secondary">
                  <span class="cell-path">
                    {#if media.current_root_folder}
                      <bdi>{media.current_root_folder}</bdi>
                    {:else}
                      <NoValue />
                    {/if}
                  </span>
                </td>
                <td class="col-secondary">{media.instance_name}</td>
              </tr>
            {/each}
          {/if}
        </tbody>
      </table>
    </TableRegion>

    <Pager {pagination} bind:page countKey="ItemCount" />
  </div>

  {#if explainer.shown}
    <ExplanationModal data={explainer.shown} onClose={explainer.close} onChanged={reload} />
  {/if}
</div>
