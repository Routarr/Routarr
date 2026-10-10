<script lang="ts">
  import CardTitle from '../components/CardTitle.svelte';
  import { CheckCircle2, RefreshCw } from '../lib/icons';
  import { api } from '../api/client';
  import { createAsync } from '../lib/async.svelte';
  import { invalidateStatus } from '../lib/status.svelte';
  import { i18n, t } from '../lib/i18n.svelte';
  import { formatCount, formatRelative, formatTimestamp } from '../api/format';
  import NoValue from '../components/NoValue.svelte';
  import Count from '../components/Count.svelte';
  import EmptyState from '../components/EmptyState.svelte';
  import ErrorBanner from '../components/ErrorBanner.svelte';
  import Loading from '../components/Loading.svelte';
  import Stat from '../components/Stat.svelte';
  import BannerList from '../components/BannerList.svelte';
  import InstanceStatus from '../components/InstanceStatus.svelte';
  import TableRegion from '../components/TableRegion.svelte';

  // A probe records what it finds, and the shell counts that among its
  // warnings without probing itself.
  const report = createAsync(async (signal) => {
    const found = await api.getHealth(undefined, signal);
    invalidateStatus();
    return found;
  });
  const health = $derived(report.data);
</script>

<div>
  <div class="page-header">
    <div>
      <h1 class="page-title">{t('Diagnostics')}</h1>
      <p class="page-subtitle">{t('DiagnosticsSubtitle')}</p>
    </div>
    <div class="flex gap-2">
      <!-- A re-check waits out every Arr that does not answer. -->
      <button
        class="btn btn-secondary"
        onclick={() => void report.reload()}
        aria-busy={report.loading}
      >
        <RefreshCw size={16} class={report.loading ? 'spin' : ''} />
        {t('Recheck')}
      </button>
    </div>
  </div>

  {#if report.loading && !health}
    <div class="card"><Loading label={t('RunningDiagnostics')} /></div>
  {:else}
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
        <BannerList
          tone="warning"
          title={t('DiagnosticWarnings', { count: health.warnings.length })}
          items={health.warnings.map((warning) => ({ text: warning.message }))}
        />
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
          <CardTitle card="ArrInstances">{t('ArrInstances')}</CardTitle>
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
                    <td><InstanceStatus status={instance.status} /></td>
                    <td class="mono">
                      {#if instance.version}{instance.version}{:else}<NoValue />{/if}
                    </td>
                    <td><Count value={instance.media_count} /></td>
                    <td><Count value={instance.mapped_root_folders} /></td>
                    <td
                      class="cell-timestamp"
                      title={formatTimestamp(instance.last_sync, i18n.language, '')}
                    >
                      {formatRelative(instance.last_sync, i18n.language, t('Never'))}
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
          <CardTitle card="HealthMetadataSources">{t('SettingMetadataProviders')}</CardTitle>
        </div>

        <!-- A list, not a grid of `Stat` tiles. Those are built for numbers, so
             the state would land in the figure slot and the source name in the
             caption, reading "connected / AniList" instead of "AniList,
             connected". The name is the information, and the state qualifies
             it. -->
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
          <span
            >{t('CachedEntries')}
            <b class="mono">{formatCount(health.metadata.cached_items, i18n.language)}</b></span
          >
          <span>
            {t('MissingMetadata')}
            <b class="mono">{formatCount(health.metadata.media_missing_metadata, i18n.language)}</b>
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
  {/if}
</div>
