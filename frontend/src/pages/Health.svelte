<script lang="ts">
  import { AlertTriangle, CheckCircle2, RefreshCw, XCircle } from '../lib/icons';
  import { api } from '../api/client';
  import { createAsync } from '../lib/async.svelte';
  import { i18n, t } from '../lib/i18n.svelte';
  import { formatTimestamp } from '../api/format';
  import EmptyState from '../components/EmptyState.svelte';
  import ErrorBanner from '../components/ErrorBanner.svelte';
  import Loading from '../components/Loading.svelte';
  import Stat from '../components/Stat.svelte';
  import TableRegion from '../components/TableRegion.svelte';

  const report = createAsync(() => api.getHealth());
  const health = $derived(report.data);
</script>

{#if report.loading && !health}
  <Loading label={t('RunningDiagnostics')} />
{:else}
  <div>
    <div class="page-header">
      <div>
        <h1 class="page-title">{t('Diagnostics')}</h1>
        <p class="page-subtitle">{t('DiagnosticsSubtitle')}</p>
      </div>
      <button class="btn btn-secondary" onclick={() => void report.reload()}>
        <RefreshCw size={16} />
        {t('Recheck')}
      </button>
    </div>

    <ErrorBanner
      message={report.error}
      onDismiss={() => (report.error = null)}
      onRetry={() => void report.reload()}
    />

    {#if health}
      {#if health.warnings.length === 0}
        <div class="banner banner-success">
          <CheckCircle2 size={16} />
          <span>{t('AllGood')}</span>
        </div>
      {:else}
        {#each health.warnings as warning, index (index)}
          <div class="banner banner-warning">
            <AlertTriangle size={16} />
            <span>{warning}</span>
          </div>
        {/each}
      {/if}

      <div class="metrics">
        <Stat label={t('MediaTracked')} value={health.stats.total_media} />
        <Stat label={t('Movies')} value={health.stats.total_movies} />
        <Stat label={t('Series')} value={health.stats.total_series} />
        <Stat label={t('ActiveRules')} value={health.stats.enabled_rules} />
        <Stat label={t('PendingDecisions')} value={health.stats.pending_decisions} tone="warning" />
        <Stat label={t('FailedDecisions')} value={health.stats.failed_decisions} tone="danger" />
        <Stat label={t('Overrides')} value={health.stats.total_overrides} />
        <Stat label={t('RunningJobs')} value={health.stats.running_jobs} />
      </div>

      <div class="card">
        <div class="card-header">
          <h2 class="card-title">{t('ArrInstances')}</h2>
        </div>
        <TableRegion label={t('ArrInstances')}>
          <table>
            <caption class="visually-hidden">{t('ArrInstances')}</caption>
            <thead>
              <tr>
                <th>{t('Instance')}</th>
                <th>{t('Type')}</th>
                <th>{t('Status')}</th>
                <th>{t('Version')}</th>
                <th>{t('Media')}</th>
                <th>{t('MappedFolders')}</th>
                <th>{t('LastSync')}</th>
              </tr>
            </thead>
            <tbody>
              {#if health.instances.length === 0}
                <tr><td colspan="7"><EmptyState>{t('NoInstanceConfigured')}</EmptyState></td></tr>
              {:else}
                {#each health.instances as instance (instance.id)}
                  <tr>
                    <td><strong>{instance.name}</strong></td>
                    <td>
                      <span class="badge badge-kind kind-{instance.instance_type}">
                        {instance.instance_type}
                      </span>
                    </td>
                    <td>
                      {#if instance.status === 'connected'}
                        <span class="flex items-center gap-2 text-success">
                          <CheckCircle2 size={14} />
                          {t('Connected')}
                        </span>
                      {:else if instance.status === 'disabled'}
                        <span class="text-muted">{t('Disabled')}</span>
                      {:else}
                        <span class="flex items-center gap-2 text-danger">
                          <XCircle size={14} />
                          {instance.status}
                        </span>
                      {/if}
                    </td>
                    <td class="mono">{instance.version ?? t('None')}</td>
                    <td>{instance.media_count}</td>
                    <td>
                      <!-- A count, not a verdict: zero mapped folders is already
                           reported in the warnings above, and it was the only
                           reason this number was ever painted red. -->
                      <span class="num">{instance.mapped_root_folders}</span>
                    </td>
                    <td class="text-muted mono text-sm">
                      {formatTimestamp(instance.last_sync, i18n.language, t('Never'))}
                    </td>
                  </tr>
                {/each}
              {/if}
            </tbody>
          </table>
        </TableRegion>
      </div>

      <div class="card">
        <div class="card-header">
          <h2 class="card-title">{t('SettingMetadataProviders')}</h2>
        </div>

        <!-- A list, not a grid of `Stat` tiles. Those are built for numbers, so
             the state landed in the figure slot and the source name in the
             caption: one read "connected / AniList" instead of "AniList —
             connected". The name is the information; the state qualifies it. -->
        <div class="source-list">
          {#each health.metadata.providers as provider (provider.id)}
            {@const waiting = !provider.configured}
            {@const broken = provider.connected === false}
            <div class="source-row{waiting ? ' waiting' : ''}">
              <!-- Name only: this screen answers "is it working". What a source
                   brings, and which variable it waits for, belongs to Settings. -->
              <div class="source-name">{provider.display_name}</div>
              <span
                class="badge {broken
                  ? 'badge-danger'
                  : waiting
                    ? 'badge-warning'
                    : 'badge-success'}"
              >
                {t(
                  broken
                    ? 'Error'
                    : waiting
                      ? 'ProviderNeedsKey'
                      : provider.connected === true
                        ? 'Connected'
                        : 'ProviderActive',
                )}
              </span>
            </div>
          {/each}
        </div>

        <!-- Apart from the sources: counts and a database probe among them
             would be three kinds of fact in one grid. The counts are counts. -->
        <div class="source-footer">
          <span>{t('CachedEntries')} <b class="mono">{health.metadata.cached_items}</b></span>
          <span>
            {t('MissingMetadata')}
            <b class="mono">{health.metadata.media_missing_metadata}</b>
          </span>
          <span>
            {t('Database')}
            <span
              class="badge {health.database === 'connected' ? 'badge-success' : 'badge-danger'}"
            >
              {t(health.database === 'connected' ? 'Connected' : 'Error')}
            </span>
          </span>
        </div>
      </div>
    {/if}
  </div>
{/if}
