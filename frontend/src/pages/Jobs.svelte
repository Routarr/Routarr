<script lang="ts">
  import { RefreshCw } from '../lib/icons';
  import { api } from '../api/client';
  import { formatTimestamp, capitalize } from '../api/format';
  import type { Job } from '../api/types';
  import { createAsync } from '../lib/async.svelte';
  import { poll } from '../lib/poll.svelte';
  import { i18n, t } from '../lib/i18n.svelte';
  import EmptyState from '../components/EmptyState.svelte';
  import ErrorBanner from '../components/ErrorBanner.svelte';
  import Loading from '../components/Loading.svelte';
  import TableRegion from '../components/TableRegion.svelte';

  /** Backend enum values are lower-case; the dictionary keys are PascalCase. */
  const jobKindKey = (kind: string) => `Job${capitalize(kind)}`;
  const triggerKey = (trigger: string) => `Trigger${capitalize(trigger)}`;
  const statusKey = (status: string) => `Status${capitalize(status)}`;

  const STATUS_BADGE: Record<Job['status'], string> = {
    running: 'badge-info',
    queued: 'badge-warning',
    success: 'badge-success',
    failed: 'badge-danger',
  };

  let status = $state('');

  const jobsPage = createAsync(
    () => api.getJobs({ status: status || undefined, per_page: 50 }),
    () => status,
  );

  const jobs = $derived(jobsPage.data?.data ?? []);
  const hasRunning = $derived(jobs.some((job) => job.status === 'running'));

  // Poll only while something is in flight, and only while the tab is in front.
  poll(
    () => void jobsPage.reload(),
    () => 3000,
    () => hasRunning,
  );
</script>

<div>
  <div class="page-header">
    <div>
      <h1 class="page-title">{t('Tasks')}</h1>
      <p class="page-subtitle">{t('TasksSubtitle')}</p>
    </div>
    <button class="btn btn-secondary" onclick={() => void jobsPage.reload()}>
      <RefreshCw size={16} />
      {t('Refresh')}
    </button>
  </div>

  <ErrorBanner
    message={jobsPage.error}
    onDismiss={() => (jobsPage.error = null)}
    onRetry={() => void jobsPage.reload()}
  />

  <div class="toolbar">
    <select class="form-select" aria-label={t('FilterByStatus')} bind:value={status}>
      <option value="">{t('AllStatuses')}</option>
      <option value="running">{t('StatusRunning')}</option>
      <option value="success">{t('StatusSuccess')}</option>
      <option value="failed">{t('StatusFailed')}</option>
    </select>
    {#if status}
      <button type="button" class="btn btn-ghost" onclick={() => (status = '')}
        >{t('ClearFilters')}</button
      >
    {/if}
    {#if hasRunning}
      <span class="badge badge-info">{t('AutoRefreshing')}</span>
    {/if}
  </div>

  <div class="card">
    <TableRegion label={t('Tasks')}>
      <table>
        <caption class="visually-hidden">{t('Tasks')}</caption>
        <thead>
          <tr>
            <th>{t('Task')}</th>
            <th>{t('Trigger')}</th>
            <th>{t('Status')}</th>
            <th>{t('Progress')}</th>
            <th>{t('Detail')}</th>
            <th>{t('Started')}</th>
            <th>{t('Finished')}</th>
          </tr>
        </thead>
        <tbody>
          {#if jobsPage.loading && jobs.length === 0}
            <tr><td colspan="7"><Loading /></td></tr>
          {:else if jobs.length === 0}
            <tr>
              <td colspan="7"><EmptyState>{t('NoTaskYet')}</EmptyState></td>
            </tr>
          {:else}
            {#each jobs as job (job.id)}
              <tr>
                <td><strong>{t(jobKindKey(job.kind))}</strong></td>
                <td><span class="badge badge-info">{t(triggerKey(job.trigger))}</span></td>
                <td>
                  <span class="badge {STATUS_BADGE[job.status]}">{t(statusKey(job.status))}</span>
                </td>
                <td>
                  {#if job.progress_total > 0}
                    <div class="flex items-center gap-2">
                      <div class="progress-track">
                        <div
                          class="progress-fill"
                          style="width: {Math.min(
                            100,
                            (job.progress_current / job.progress_total) * 100,
                          )}%"
                        ></div>
                      </div>
                      <span class="mono text-sm">
                        {job.progress_current}/{job.progress_total}
                      </span>
                    </div>
                  {:else}
                    <span class="text-muted">{t('None')}</span>
                  {/if}
                </td>
                <td class="max-w-320">
                  {#if job.error_message}
                    <span class="text-danger">{job.error_message}</span>
                  {:else}
                    <span class="text-muted">{job.detail ?? t('None')}</span>
                  {/if}
                </td>
                <td class="cell-timestamp" title={job.started_at}>
                  {formatTimestamp(job.started_at, i18n.language)}
                </td>
                <td class="cell-timestamp" title={job.finished_at ?? undefined}>
                  {formatTimestamp(job.finished_at, i18n.language, t('None'))}
                </td>
              </tr>
            {/each}
          {/if}
        </tbody>
      </table>
    </TableRegion>
  </div>
</div>
