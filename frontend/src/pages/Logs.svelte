<script lang="ts">
  import { Download, RefreshCw } from '../lib/icons';
  import { api, getApiKey } from '../api/client';
  import { formatTimestamp } from '../api/format';
  import { createAsync } from '../lib/async.svelte';
  import { createOutcome } from '../lib/outcome.svelte';
  import { i18n, t } from '../lib/i18n.svelte';
  import EmptyState from '../components/EmptyState.svelte';
  import ErrorBanner from '../components/ErrorBanner.svelte';
  import OutcomeBanner from '../components/OutcomeBanner.svelte';
  import Loading from '../components/Loading.svelte';
  import TableRegion from '../components/TableRegion.svelte';
  import SearchField from '../components/SearchField.svelte';
  import { downloadBlob } from '../lib/download';

  let search = $state('');
  let outcome = $state('');
  let page = $state(1);

  const filters = $derived({
    search: search || undefined,
    success: outcome === '' ? undefined : outcome === 'success',
    page,
    per_page: 50,
  });

  const logs = createAsync(
    (signal) => api.getLogs(filters, signal),
    () => [search, outcome, page],
  );
  // How the last export went: `outcome` is the filter on the log itself.
  const exported = createOutcome();

  const entries = $derived(logs.data?.data ?? []);
  const pagination = $derived(logs.data?.pagination);

  /**
   * Fetch rather than link: the export needs the API key header, which a plain
   * `<a href>` cannot carry.
   */
  async function download() {
    try {
      const res = await fetch(api.logsExportUrl(filters), {
        headers: getApiKey() ? { 'X-Api-Key': getApiKey() } : {},
      });
      if (!res.ok) throw new Error(t('ExportFailed', { status: res.status }));
      downloadBlob(await res.blob(), 'routarr-logs.csv');
      exported.clear();
    } catch (err) {
      exported.fail(err);
    }
  }
</script>

<div>
  <div class="page-header">
    <div>
      <h1 class="page-title">{t('LogsTitle')}</h1>
      <p class="page-subtitle">{t('LogsSubtitle')}</p>
    </div>
    <div class="flex gap-2">
      <button class="btn btn-secondary" onclick={() => void logs.reload()}>
        <RefreshCw size={16} />
        {t('Refresh')}
      </button>
      <button class="btn btn-secondary" onclick={download} disabled={entries.length === 0}>
        <Download size={16} />
        {t('ExportCsv')}
      </button>
    </div>
  </div>

  <ErrorBanner
    message={logs.error}
    onDismiss={() => (logs.error = null)}
    onRetry={() => void logs.reload()}
  />
  <OutcomeBanner outcome={exported} />

  <div class="toolbar">
    <SearchField
      bind:value={search}
      placeholder={t('SearchTitleOrDetails')}
      label={t('SearchTitleOrDetails')}
      oninput={() => (page = 1)}
      debounce={200}
    />
    <select
      class="form-select"
      aria-label={t('FilterByOutcome')}
      value={outcome}
      onchange={(event) => {
        page = 1;
        outcome = event.currentTarget.value;
      }}
    >
      <option value="">{t('AllOutcomes')}</option>
      <option value="success">{t('OutcomeSuccess')}</option>
      <option value="failure">{t('OutcomeFailure')}</option>
    </select>
    {#if search || outcome}
      <button
        type="button"
        class="btn btn-ghost"
        onclick={() => {
          search = '';
          outcome = '';
          page = 1;
        }}>{t('ClearFilters')}</button
      >
    {/if}
  </div>

  <div class="card">
    <TableRegion label={t('LogsTitle')}>
      <table>
        <caption class="visually-hidden">{t('LogsTitle')}</caption>
        <thead>
          <tr>
            <th>{t('When')}</th>
            <th>{t('Action')}</th>
            <th>{t('Media')}</th>
            <th>{t('Details')}</th>
            <th>{t('Outcome')}</th>
          </tr>
        </thead>
        <tbody>
          {#if logs.loading && entries.length === 0}
            <tr><td colspan="5"><Loading /></td></tr>
          {:else if entries.length === 0}
            <tr><td colspan="5"><EmptyState>{t('NoWritesYet')}</EmptyState></td></tr>
          {:else}
            {#each entries as entry (entry.id)}
              <tr>
                <td class="cell-timestamp" title={entry.executed_at}>
                  {formatTimestamp(entry.executed_at, i18n.language)}
                </td>
                <td><span class="badge badge-value muted">{entry.action}</span></td>
                <td>{entry.media_title ?? t('None')}</td>
                <td class="mono text-sm">
                  {#if entry.error_message}
                    <span class="text-danger">{entry.error_message}</span>
                  {:else}
                    {entry.details}
                  {/if}
                </td>
                <td>
                  <span class="badge {entry.success ? 'badge-success' : 'badge-danger'}">
                    {t(entry.success ? 'StatusSuccess' : 'StatusFailed')}
                  </span>
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
          {t('PageOf', { page: pagination.page, total: pagination.total_pages })} — {t(
            'LogEntryCount',
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
</div>
