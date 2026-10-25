<script lang="ts">
  import { Film, Lock, Tv } from '../lib/icons';
  import { api } from '../api/client';
  import { mediaTypeKey } from '../api/format';
  import { createAsync } from '../lib/async.svelte';
  import { createExplainer } from '../lib/explain.svelte';
  import { cut, t } from '../lib/i18n.svelte';
  import NoValue from '../components/NoValue.svelte';
  import EmptyState from '../components/EmptyState.svelte';
  import ErrorBanner from '../components/ErrorBanner.svelte';
  import ExplanationModal from '../components/ExplanationModal.svelte';
  import WhyButton from '../components/WhyButton.svelte';
  import TableSkeleton from '../components/TableSkeleton.svelte';
  import TableRegion from '../components/TableRegion.svelte';
  import Pager from '../components/Pager.svelte';
  import SearchField from '../components/SearchField.svelte';

  let search = $state('');
  let mediaType = $state('');
  let unmatched = $state(false);
  const filtered = $derived(Boolean(search || mediaType || unmatched));
  let page = $state(1);

  const library = createAsync(
    (signal) =>
      api.getLibrary(
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
  const explainer = createExplainer();

  const items = $derived(library.data?.data ?? []);
  const pagination = $derived(library.data?.pagination);

  /** The year after the title as the language sets a note, its brackets muted with it. */
  const yearNote = $derived(cut('WithNote', ['text', 'note']));
</script>

<div>
  <div class="page-header">
    <div>
      <h1 class="page-title">{t('Library')}</h1>
      <p class="page-subtitle">{t('LibrarySubtitle')}</p>
    </div>
  </div>

  <ErrorBanner
    message={library.error}
    onDismiss={() => (library.error = null)}
    onRetry={() => void library.reload()}
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
    <!-- Offered only while a filter is active: a button that does nothing is noise. -->
    {#if filtered}
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
    <TableRegion label={t('Library')}>
      <table>
        <caption class="visually-hidden">{t('Library')}</caption>
        <thead>
          <tr>
            <th>{t('Title')}</th>
            <th>{t('Instance')}</th>
            <th>{t('CurrentRootFolder')}</th>
            <th>{t('ProposedCategory')}</th>
            <th>{t('Metadata')}</th>
          </tr>
        </thead>
        <tbody>
          {#if library.loading && items.length === 0}
            <TableSkeleton columns={5} />
          {:else if items.length === 0 && !library.error}
            <tr>
              <td colspan="5">
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
                        {@const [before, between, after] = yearNote}
                        {before}<strong>{media.title}</strong><span class="text-muted"
                          >{between}{media.year}{after}</span
                        >
                      {:else}
                        <strong>{media.title}</strong>
                      {/if}
                    </span>
                    <WhyButton {explainer} mediaId={media.id} title={media.title} />
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
                      <span class="visually-hidden">{t('ManualOverride')}</span>
                    </span>
                  {:else if media.computed_category}
                    <span class="badge badge-value">{media.computed_category}</span>
                  {:else}
                    <!-- A pill either way: a pill beside grey prose reads as two kinds
                         of data in one column. -->
                    <span class="badge badge-value muted">{t('NotEvaluated')}</span>
                  {/if}
                </td>
                <td>
                  <!-- Having metadata is not an outcome, so it carries no status colour.
                       Missing it stops genre and keyword rules matching: a warning. -->
                  <span class="badge {media.has_metadata ? 'badge-value muted' : 'badge-warning'}">
                    {t(media.has_metadata ? 'MetadataCached' : 'MetadataMissing')}
                  </span>
                </td>
              </tr>
            {/each}
          {/if}
        </tbody>
      </table>
    </TableRegion>

    <Pager {pagination} bind:page countKey="ItemCount" />
  </div>

  {#if explainer.shown}
    <ExplanationModal
      data={explainer.shown}
      onClose={explainer.close}
      onChanged={() => void library.reload()}
    />
  {/if}
</div>
