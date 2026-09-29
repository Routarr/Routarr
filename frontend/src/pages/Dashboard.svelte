<script lang="ts">
  import { Play, RefreshCw } from '../lib/icons';
  import { api } from '../api/client';
  import { createAsync } from '../lib/async.svelte';
  import { href } from '../lib/router.svelte';
  import { i18n, t } from '../lib/i18n.svelte';
  import { formatRelative, formatTimestamp } from '../api/format';
  import { createOutcome } from '../lib/outcome.svelte';
  import { onboarding } from '../lib/onboarding.svelte';
  import { invalidateStatus } from '../lib/status.svelte';
  import { outsideTheGuide } from '../api/onboarding';
  import ErrorBanner from '../components/ErrorBanner.svelte';
  import BannerList from '../components/BannerList.svelte';
  import EmptyState from '../components/EmptyState.svelte';
  import GettingStarted from '../components/GettingStarted.svelte';
  import InstanceStatus from '../components/InstanceStatus.svelte';
  import Loading from '../components/Loading.svelte';
  import OutcomeBanner from '../components/OutcomeBanner.svelte';
  import Stat from '../components/Stat.svelte';
  import TableRegion from '../components/TableRegion.svelte';

  /**
   * Two requests, on purpose.
   *
   * The first answers from the database and lands in milliseconds. The second
   * probes every Arr over the network, which waits out a connect timeout when
   * one is down, and an Arr being down is exactly why somebody opens this page.
   * So the page renders on the first and upgrades to the second when it arrives:
   * the instance table shows what is known immediately and fills its status
   * column in behind.
   */
  const quick = createAsync((signal) => api.getHealth({ probe: false }, signal));
  const probed = createAsync((signal) => api.getHealth(undefined, signal));

  const health = $derived(probed.data ?? quick.data);
  const outcome = createOutcome();
  // A warning an open step restates is left to the guide above it: listing
  // both says the same thing twice, the second time as an alarm. It stays on
  // the diagnostics screen.
  const warnings = $derived(outsideTheGuide(health?.warnings ?? [], onboarding.current));
  const stats = $derived(health?.stats);

  async function refresh() {
    await Promise.all([quick.reload(), probed.reload()]);
  }
</script>

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
    </div>
  </div>

  {#if quick.loading && !health}
    <div class="card"><Loading /></div>
  {:else}
    <ErrorBanner
      message={quick.error ?? probed.error}
      onDismiss={() => ((quick.error = null), (probed.error = null))}
    />
    <OutcomeBanner {outcome} />

    <!-- The shell reads the guide, and this is where the guide lives: a read
         that failed is said here, rather than the guide going missing. -->
    <ErrorBanner
      message={onboarding.failure ? t('GuideUnavailable', { error: onboarding.failure }) : null}
      onRetry={invalidateStatus}
    />
    {#if onboarding.current}
      <GettingStarted status={onboarding.current} {outcome} />
    {/if}

    <!-- One block, not one banner per warning: stacked bars would say the same
         thing once per warning, each with its own copy of the same button. The
         count leads, so the warnings past the three listed are still counted,
         and the one action sits once. -->
    <BannerList
      tone="warning"
      title={t('DiagnosticWarnings', { count: warnings.length })}
      items={warnings.slice(0, 3).map((warning) => ({ text: warning.message }))}
    >
      {#snippet action()}
        <a href={href('/health')} class="btn btn-secondary btn-sm">{t('Diagnostics')}</a>
      {/snippet}
    </BannerList>

    {#if stats && health}
      <!-- A dashboard answers one question first. Cards of equal weight, some
           with a coloured icon tile and some colouring their *number* by
           sentiment, would be two encoding systems on one row with nothing
           saying where to look. -->
      <div class="headline">
        <div>
          <div class="headline-value">{stats.pending_decisions}</div>
          <div class="headline-label">{t('PendingDecisions')}</div>
        </div>
        <!-- The one primary action of the screen, unless the guide shows:
             its next step is then the one. -->
        <a
          href={href('/simulation')}
          class="btn {onboarding.current?.state === 'pending' ? 'btn-secondary' : 'btn-primary'}"
        >
          <Play size={16} />
          {t('Simulation')}
        </a>
      </div>

      <!-- Everything else is context, so it reads as one line of facts rather
           than as competing cards. -->
      <div class="metrics">
        <Stat label={t('MoviesManaged')} value={stats.total_movies} />
        <Stat label={t('SeriesManaged')} value={stats.total_series} />
        <Stat label={t('ActiveRules')} value={stats.enabled_rules} />
        <Stat label={t('MovesApplied')} value={stats.applied_decisions} />
        <!-- The one number that changes colour, because a failure is the only
             one of these that asks for something. -->
        <Stat label={t('FailedMovesLabel')} value={stats.failed_decisions} tone="danger" />
      </div>

      <!-- A sentence does not need a card and a heading: that spends a card's
           height on one line, between two blocks that carry actual
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
                  <td><InstanceStatus status={instance.status} /></td>
                  <td>{instance.media_count}</td>
                  <td>{instance.mapped_root_folders}</td>
                  <td
                    class="cell-timestamp"
                    title={formatTimestamp(instance.last_sync, i18n.language, '')}
                  >
                    {formatRelative(instance.last_sync, i18n.language, t('Never'))}
                  </td>
                </tr>
              {/each}
              {#if health.instances.length === 0}
                <tr>
                  <td colspan="5"><EmptyState>{t('NoInstanceConfigured')}</EmptyState></td>
                </tr>
              {/if}
            </tbody>
          </table>
        </TableRegion>
      </div>
    {/if}
  {/if}
</div>
