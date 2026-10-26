<script lang="ts">
  import CardTitle from '../components/CardTitle.svelte';
  import { Play, RefreshCw } from '../lib/icons';
  import { api } from '../api/client';
  import { createAsync } from '../lib/async.svelte';
  import { poll } from '../lib/poll.svelte';
  import { href } from '../lib/router.svelte';
  import { i18n, t } from '../lib/i18n.svelte';
  import { formatCount, formatRelative, formatTimestamp } from '../api/format';
  import { createOutcome } from '../lib/outcome.svelte';
  import { onboarding } from '../lib/onboarding.svelte';
  import { access } from '../lib/access.svelte';
  import { invalidateStatus } from '../lib/status.svelte';
  import { outsideTheGuide } from '../api/onboarding';
  import { fixOf } from '../lib/warnings';
  import { screenKey } from '../lib/routes';
  import Count from '../components/Count.svelte';
  import ErrorBanner from '../components/ErrorBanner.svelte';
  import BannerList from '../components/BannerList.svelte';
  import GettingStarted from '../components/GettingStarted.svelte';
  import InstanceStatus from '../components/InstanceStatus.svelte';
  import Loading from '../components/Loading.svelte';
  import OutcomeBanner from '../components/OutcomeBanner.svelte';
  import Stat from '../components/Stat.svelte';
  import TableRegion from '../components/TableRegion.svelte';

  /**
   * Two requests: the first answers from the database at once, the second probes every Arr and
   * waits out a connect timeout on one that is down, the reason this page is opened. The page
   * draws on the first and fills the status column from the second.
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
  // Its choices write, which a reader is refused.
  const guided = $derived(onboarding.current?.state === 'pending' && !access.readOnly);
  /** The library's titles a source describes, of every title the instances hold. */
  const described = $derived(
    stats && health ? stats.total_media - health.metadata.media_missing_metadata : 0,
  );

  async function refresh() {
    await Promise.all([quick.reload(), probed.reload()]);
  }
  // Its relative times and counts follow on their own while it is open.
  poll(
    () => void refresh(),
    () => 60_000,
  );
</script>

<div>
  <div class="page-header">
    <div>
      <h1 class="page-title">{t('Dashboard')}</h1>
      <p class="page-subtitle">{t('DashboardSubtitle')}</p>
    </div>
    <div class="flex gap-2">
      <button
        class="btn btn-secondary"
        onclick={() => void refresh()}
        aria-busy={quick.loading || probed.loading}
      >
        <RefreshCw size={16} class={quick.loading || probed.loading ? 'spin' : ''} />
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
      onRetry={() => void refresh()}
    />
    <OutcomeBanner {outcome} />

    <!-- The shell reads the guide, and this is where the guide lives: a read
         that failed is said here, rather than the guide going missing. -->
    <ErrorBanner
      message={onboarding.failure ? t('GuideUnavailable', { error: onboarding.failure }) : null}
      onRetry={invalidateStatus}
    />
    {#if onboarding.current && !access.readOnly}
      <GettingStarted status={onboarding.current} {outcome} />
    {/if}

    <!-- The count leads, so the warnings past the three listed are still counted. -->
    <BannerList
      tone="warning"
      title={t('DiagnosticWarnings', { count: warnings.length })}
      items={warnings
        .slice(0, 3)
        .map((warning) => ({ text: warning.message, link: fixOf(warning) }))}
    >
      {#snippet action()}
        <a href={href('/diagnostics')} class="btn btn-secondary btn-sm">{t('Diagnostics')}</a>
      {/snippet}
    </BannerList>

    {#if stats && health && health.instances.length === 0}
      <!-- Before the first instance every figure is zero: the guide, or this, says what next. -->
      {#if !guided}
        <div class="card">
          <div class="empty-state">
            <span>{t('DashboardStartHere')}</span>
            {#if !access.readOnly}
              <a href={href('/instances?add=1')} class="btn btn-primary"
                >{t('GuideInstanceAction')}</a
              >
            {/if}
          </div>
        </div>
      {/if}
    {:else if stats && health}
      <div class="headline">
        <div>
          <div class="headline-value">{formatCount(stats.pending_moves, i18n.language)}</div>
          <div class="headline-label">{t('MovesToReview')}</div>
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

      <!-- The library's titles both times: the sources' cache counts what they hold, which
           Diagnostics shows. -->
      {#if stats.total_media > 0}
        <p class="page-note">
          <span class={described < stats.total_media ? 'text-warning' : 'text-muted'}
            >{t('MetadataCoverage', { described, total: stats.total_media })}</span
          >
          {#if described < stats.total_media}
            <a class="text-link ms-2" href={href('/sources')}
              >{t('HelpFixOn', { screen: t(screenKey('/sources')) })}</a
            >
          {/if}
        </p>
      {/if}

      <div class="card">
        <div class="card-header">
          <CardTitle card="DashboardInstances">{t('Instances')}</CardTitle>
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
                    <!-- Still unchecked once the probe failed is not being checked, nor
                         for a reader, whose read the server answers without a probe. -->
                    <InstanceStatus
                      status={(probed.error || access.readOnly) && instance.status === 'unchecked'
                        ? 'unknown'
                        : instance.status}
                      detail={instance.detail}
                    />
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
            </tbody>
          </table>
        </TableRegion>
      </div>
    {/if}
  {/if}
</div>
