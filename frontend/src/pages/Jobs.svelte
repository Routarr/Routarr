<script lang="ts">
  import { RefreshCw } from '../lib/icons';
  import { api } from '../api/client';
  import { formatTimestamp, capitalize, statusKey, triggerKey } from '../api/format';
  import type { Job } from '../api/types';
  import { createAsync } from '../lib/async.svelte';
  import { poll } from '../lib/poll.svelte';
  import { i18n, t } from '../lib/i18n.svelte';
  import EmptyState from '../components/EmptyState.svelte';
  import ErrorBanner from '../components/ErrorBanner.svelte';
  import TableSkeleton from '../components/TableSkeleton.svelte';
  import TableRegion from '../components/TableRegion.svelte';
  import Pager from '../components/Pager.svelte';

  /** Backend enum values are lower-case, and the dictionary keys are PascalCase. */
  // `sync_all` reads `JobSyncAll`, as the backend's test of the labels builds it.
  const jobKindKey = (kind: string) => `Job${kind.split('_').map(capitalize).join('')}`;

  const STATUS_BADGE: Record<Job['status'], string> = {
    running: 'badge-info',
    success: 'badge-success',
    failed: 'badge-danger',
  };

  let status = $state('');
  // A sync every 15 minutes fills a page of 50 in half a day, and last night's
  // failure is then reachable only through a filter nobody thinks of.
  let page = $state(1);

  const jobsPage = createAsync(
    (signal) => api.getJobs({ status: status || undefined, page, per_page: 50 }, signal),
    () => [status, page],
  );
  const pagination = $derived(jobsPage.data?.pagination);

  const jobs = $derived(jobsPage.data?.data ?? []);
  const hasRunning = $derived(jobs.some((job) => job.status === 'running'));

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
    <div class="flex gap-2">
      <button class="btn btn-secondary" onclick={() => void jobsPage.reload()}>
        <RefreshCw size={16} />
        {t('Refresh')}
      </button>
    </div>
  </div>

  <ErrorBanner
    message={jobsPage.error}
    onDismiss={() => (jobsPage.error = null)}
    onRetry={() => void jobsPage.reload()}
  />

  <div class="toolbar">
    <select
      class="form-select"
      aria-label={t('FilterByStatus')}
      bind:value={status}
      onchange={() => (page = 1)}
    >
      <option value="">{t('AllStatuses')}</option>
      <option value="running">{t('StatusRunning')}</option>
      <option value="success">{t('StatusSuccess')}</option>
      <option value="failed">{t('StatusFailed')}</option>
    </select>
    {#if status}
      <button type="button" class="btn btn-ghost" onclick={() => ((status = ''), (page = 1))}
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
            <TableSkeleton columns={7} />
          {:else if jobs.length === 0 && !jobsPage.error}
            <tr>
              <td colspan="7"><EmptyState>{t('NoTaskYet')}</EmptyState></td>
            </tr>
          {:else}
            {#each jobs as job (job.id)}
              <tr>
                <td><strong>{t(jobKindKey(job.kind))}</strong></td>
                <td>
                  <span class="badge badge-info">{t(triggerKey(job.trigger))}</span>
                  {#if job.subject}
                    <span class="text-xs text-muted" title={t('PerformedBy')}>{job.subject}</span>
                  {/if}
                </td>
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
                  <!-- A task that fails on its own count, such as every move of
                       an apply refused, says so in its detail and has no error. -->
                  {#if job.error_message}
                    <span class="text-danger">{job.error_message}</span>
                  {:else}
                    <span class={job.status === 'failed' ? 'text-danger' : 'text-muted'}>
                      {job.detail ?? t('None')}
                    </span>
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

    <Pager {pagination} bind:page countKey="TaskCount" />
  </div>
</div>
