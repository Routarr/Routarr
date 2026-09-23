<script lang="ts">
  import { AlertTriangle, Play, RefreshCw } from '../lib/icons';
  import { api } from '../api/client';
  import { createAsync } from '../lib/async.svelte';
  import { href } from '../lib/router.svelte';
  import { i18n, t } from '../lib/i18n.svelte';
  import { formatTimestamp } from '../api/format';
  import ErrorBanner from '../components/ErrorBanner.svelte';
  import Loading from '../components/Loading.svelte';
  import TableRegion from '../components/TableRegion.svelte';

  /**
   * Two requests, on purpose.
   *
   * The first answers from the database and lands in milliseconds. The second
   * probes every Arr over the network, which takes five seconds when one is
   * down — and an Arr being down is exactly why somebody opens this page. So
   * the page renders on the first and upgrades to the second when it arrives:
   * the instance table shows what is known immediately and fills its status
   * column in behind.
   */
  const quick = createAsync((signal) => api.getHealth({ probe: false }, signal));
  const probed = createAsync((signal) => api.getHealth(undefined, signal));

  const health = $derived(probed.data ?? quick.data);
  const stats = $derived(health?.stats);

  async function refresh() {
    await Promise.all([quick.reload(), probed.reload()]);
  }
</script>

{#if quick.loading && !health}
  <Loading />
{:else}
  <div>
    <div class="page-header">
      <div>
        <h1 class="page-title">{t('Dashboard')}</h1>
        <p class="page-subtitle">{t('DashboardSubtitle')}</p>
      </div>
      <div class="flex gap-2">
        <button class="btn btn-secondary" onclick={() => void refresh()}>
          <RefreshCw size={16} />
          {t('Refresh')}
        </button>
        <a href={href('/simulation')} class="btn btn-primary">
          <Play size={16} />
          {t('RunADryRun')}
        </a>
      </div>
    </div>

    <ErrorBanner
      message={quick.error ?? probed.error}
      onDismiss={() => ((quick.error = null), (probed.error = null))}
    />

    <!-- One block, not one banner per warning. Three stacked tinted bars said
         the same thing three times, each with its own copy of the same button,
         and a fourth warning was invisible because the list was capped at
         three without saying so. The count leads, the list follows, and the
         one action sits once. -->
    {#if health && health.warnings.length > 0}
      <div class="banner banner-warning items-start">
        <AlertTriangle size={16} />
        <div class="flex-1">
          <div class="flex items-center justify-between gap-2">
            <strong>{t('DiagnosticWarnings', { count: health.warnings.length })}</strong>
            <a href={href('/health')} class="btn btn-secondary btn-sm">{t('Diagnostics')}</a>
          </div>
          <ul class="banner-list">
            {#each health.warnings.slice(0, 3) as warning, index (index)}
              <li>{warning}</li>
            {/each}
          </ul>
        </div>
      </div>
    {/if}

    {#if stats && health}
      <!-- A dashboard answers one question first. Six cards of equal weight,
           some with a coloured icon tile and some colouring their *number* by
           sentiment, are two encoding systems on one row with nothing saying
           where to look. -->
      <div class="headline">
        <div>
          <div class="headline-value">{stats.pending_decisions}</div>
          <div class="headline-label">{t('PendingDecisions')}</div>
        </div>
        <a href={href('/simulation')} class="btn btn-primary">
          <Play size={16} />
          {t('Simulation')}
        </a>
      </div>

      <!-- Everything else is context, so it reads as one line of facts rather
           than as six competing cards. -->
      <div class="metrics">
        <div class="metric">
          <span class="metric-value">{stats.total_movies}</span>
          <span class="metric-label">{t('MoviesManaged')}</span>
        </div>
        <div class="metric">
          <span class="metric-value">{stats.total_series}</span>
          <span class="metric-label">{t('SeriesManaged')}</span>
        </div>
        <div class="metric">
          <span class="metric-value">{stats.enabled_rules}</span>
          <span class="metric-label">{t('ActiveRules')}</span>
        </div>
        <div class="metric">
          <span class="metric-value">{stats.applied_decisions}</span>
          <span class="metric-label">{t('MovesApplied')}</span>
        </div>
        <!-- The one number that changes colour, because a failure is the only
             one of these that asks for something. -->
        <div class="metric{stats.failed_decisions > 0 ? ' is-alert' : ''}">
          <span class="metric-value">{stats.failed_decisions}</span>
          <span class="metric-label">{t('FailedMovesLabel')}</span>
        </div>
      </div>

      <!-- A sentence does not need a card and a heading: that spends 100px of
           vertical space on one line, between two blocks that carry actual
           structure. -->
      <p class="page-note">
        {t('MetadataEnrichedCount', { count: health.metadata.cached_items })}
        {#if health.metadata.media_missing_metadata > 0}
          <span class="text-muted">
            {t('MetadataMissingCount', { count: health.metadata.media_missing_metadata })}
          </span>
        {:else}
          <span class="text-success">{t('MetadataComplete')}</span>
        {/if}
      </p>

      <div class="card">
        <div class="card-header">
          <h2 class="card-title">{t('Instances')}</h2>
          <a href={href('/instances')} class="btn btn-secondary btn-sm">{t('Manage')}</a>
        </div>
        <TableRegion label={t('Instances')}>
          <table>
            <caption class="visually-hidden">{t('Instances')}</caption>
            <thead>
              <tr>
                <th>{t('Instance')}</th>
                <th>{t('Status')}</th>
                <th>{t('Media')}</th>
                <th>{t('MappedFolders')}</th>
                <th>{t('LastSync')}</th>
              </tr>
            </thead>
            <tbody>
              {#each health.instances as instance (instance.id)}
                <tr>
                  <td>
                    <strong>{instance.name}</strong>
                    <span class="badge badge-kind kind-{instance.instance_type}">
                      {instance.instance_type}
                    </span>
                  </td>
                  <td>
                    {#if instance.status === 'unchecked'}
                      <!-- The probe has not answered yet. Saying so beats
                           showing a state that is not known, and beats an
                           empty cell that reads as "no instance". -->
                      <span class="badge badge-value muted">{t('Checking')}</span>
                    {:else}
                      <span
                        class="badge {instance.status === 'connected'
                          ? 'badge-success'
                          : 'badge-danger'}"
                      >
                        {instance.status === 'connected'
                          ? t('Connected')
                          : instance.status === 'disabled'
                            ? t('Disabled')
                            : instance.status}
                      </span>
                    {/if}
                  </td>
                  <td>{instance.media_count}</td>
                  <td>{instance.mapped_root_folders}</td>
                  <td class="cell-timestamp text-muted" title={instance.last_sync ?? undefined}>
                    {formatTimestamp(instance.last_sync, i18n.language, t('Never'))}
                  </td>
                </tr>
              {/each}
              {#if health.instances.length === 0}
                <tr>
                  <td colspan="5" class="text-muted text-center">
                    {t('NoInstanceYet')}
                  </td>
                </tr>
              {/if}
            </tbody>
          </table>
        </TableRegion>
      </div>
    {/if}
  </div>
{/if}
