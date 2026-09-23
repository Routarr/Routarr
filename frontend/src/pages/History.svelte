<script lang="ts">
  import { Undo2 } from '../lib/icons';
  import { api } from '../api/client';
  import { formatTimestamp, capitalize } from '../api/format';
  import type { Decision } from '../api/types';
  import { createAsync, describeError } from '../lib/async.svelte';
  import { i18n, t } from '../lib/i18n.svelte';
  import Confidence from '../components/Confidence.svelte';
  import EmptyState from '../components/EmptyState.svelte';
  import ErrorBanner from '../components/ErrorBanner.svelte';
  import Modal from '../components/Modal.svelte';
  import SuccessBanner from '../components/SuccessBanner.svelte';
  import TableSkeleton from '../components/TableSkeleton.svelte';
  import TableRegion from '../components/TableRegion.svelte';
  import SearchField from '../components/SearchField.svelte';

  const STATUS_KEY: Record<string, string> = {
    applied: 'StatusApplied',
    pending: 'StatusPending',
    failed: 'StatusFailed',
    skipped: 'StatusSkipped',
  };

  const STATUS_TONE: Record<string, string> = {
    applied: 'badge-success',
    pending: 'badge-warning',
    failed: 'badge-danger',
    skipped: 'badge-info',
  };

  let status = $state('');
  let search = $state('');
  let includeSuperseded = $state(false);
  let page = $state(1);
  let notice = $state<string | null>(null);

  const history = createAsync(
    (signal) =>
      api.getDecisions(
        {
          status: status || undefined,
          search: search || undefined,
          include_superseded: includeSuperseded,
          page,
          per_page: 50,
        },
        signal,
      ),
    () => [status, search, includeSuperseded, page],
  );

  const decisions = $derived(history.data?.data ?? []);
  const pagination = $derived(history.data?.pagination);

  // One dialog asks both questions. Chaining two — "revert?", then "move the
  // files too?" — gives the second no context, and cancelling it would mean
  // "revert without moving files" rather than "stop": a Cancel button that does
  // not cancel. Here Cancel means cancel.
  let reverting = $state<Decision | null>(null);
  let revertFiles = $state(false);

  async function revert(decision: Decision, moveFiles: boolean) {
    reverting = null;
    try {
      const report = await api.revertDecisions([decision.id], moveFiles);
      notice =
        report.applied > 0
          ? t('RevertResult', { count: report.applied })
          : t('RevertNothing') + (report.errors[0] ? `: ${report.errors[0].message}` : '.');
      await history.reload();
    } catch (err) {
      history.error = describeError(err);
    }
  }
  /**
   * `manual` reads as `TriggerManual`, the key the Tasks screen already
   * uses. Written here rather than inline because `noUncheckedIndexedAccess`
   * makes a first character an Option and a template literal a liability.
   */
  const triggerKey = (actor: string) => `Trigger${capitalize(actor)}`;
</script>

<div>
  <div class="page-header">
    <div>
      <h1 class="page-title">{t('AuditHistory')}</h1>
      <p class="page-subtitle">{t('HistorySubtitle')}</p>
    </div>
  </div>

  <ErrorBanner
    message={history.error}
    onDismiss={() => (history.error = null)}
    onRetry={() => void history.reload()}
  />
  <SuccessBanner message={notice} />

  <div class="toolbar">
    <SearchField
      bind:value={search}
      placeholder={t('SearchATitle')}
      label={t('SearchATitle')}
      oninput={() => (page = 1)}
      debounce={200}
    />
    <select
      class="form-select"
      aria-label={t('FilterByStatus')}
      value={status}
      onchange={(event) => {
        page = 1;
        status = event.currentTarget.value;
      }}
    >
      <option value="">{t('AllStatuses')}</option>
      <option value="pending">{t('StatusPending')}</option>
      <option value="applied">{t('StatusApplied')}</option>
      <option value="failed">{t('StatusFailed')}</option>
      <option value="skipped">{t('StatusSkipped')}</option>
    </select>
    <label class="flex items-center gap-2 text-md">
      <input
        type="checkbox"
        checked={includeSuperseded}
        onchange={(event) => {
          page = 1;
          includeSuperseded = event.currentTarget.checked;
        }}
      />
      {t('IncludeSuperseded')}
    </label>
    {#if search || status || includeSuperseded}
      <button
        type="button"
        class="btn btn-ghost"
        onclick={() => {
          search = '';
          status = '';
          includeSuperseded = false;
          page = 1;
        }}>{t('ClearFilters')}</button
      >
    {/if}
  </div>

  <div class="card">
    <TableRegion label={t('AuditHistory')}>
      <table>
        <caption class="visually-hidden">{t('AuditHistory')}</caption>
        <thead>
          <tr>
            <th>{t('When')}</th>
            <th>{t('Media')}</th>
            <th>{t('Category')}</th>
            <th>{t('TargetRoot')}</th>
            <th>{t('Rule')}</th>
            <th class="w-90">{t('Confidence')}</th>
            <th>{t('Status')}</th>
            <th class="w-70">{t('Actions')}</th>
          </tr>
        </thead>
        <tbody>
          {#if history.loading && decisions.length === 0}
            <TableSkeleton columns={8} />
          {:else if decisions.length === 0}
            <tr><td colspan="8"><EmptyState>{t('NoDecisionRecorded')}</EmptyState></td></tr>
          {:else}
            {#each decisions as decision (decision.id)}
              <tr class:row-muted={decision.superseded}>
                <td class="text-muted cell-timestamp" title={decision.decided_at}>
                  {formatTimestamp(decision.decided_at, i18n.language)}
                  <!-- Null on rows written before the column existed, and a
                       guess there would read as a fact. -->
                  {#if decision.actor}
                    <span class="text-xs" title={t('TriggeredBy')}
                      >{t(triggerKey(decision.actor))}</span
                    >
                  {/if}
                  <!-- Beside the trigger, not instead of it: "manual" and who
                       answer different questions, and the scheduler answers
                       only the first. -->
                  {#if decision.subject}
                    <span class="text-xs text-muted" title={t('PerformedBy')}
                      >{decision.subject}</span
                    >
                  {/if}
                </td>
                <td>
                  <strong>{decision.media_title}</strong>
                  <div class="text-muted text-sm">{decision.instance_name}</div>
                </td>
                <td><span class="badge badge-value">{decision.target_category}</span></td>
                <td class="mono text-sm">
                  {decision.target_root_folder ?? t('None')}
                </td>
                <td>{decision.matched_rule_name ?? t('DefaultCategoryFallback')}</td>
                <td><Confidence value={decision.confidence} /></td>
                <td>
                  <span class="badge {STATUS_TONE[decision.status] ?? 'badge-info'}">
                    {t(STATUS_KEY[decision.status] ?? 'Status')}
                  </span>
                  {#if decision.superseded}
                    <div class="text-muted text-xs">{t('Superseded')}</div>
                  {/if}
                  {#if decision.reverted_at}
                    <div class="text-muted text-xs">{t('Reverted')}</div>
                  {/if}
                  {#if decision.error_message}
                    <div class="text-danger text-xs">{decision.error_message}</div>
                  {/if}
                </td>
                <td>
                  {#if decision.status === 'applied' && !decision.reverted_at}
                    <button
                      class="btn btn-secondary btn-sm"
                      onclick={() => {
                        revertFiles = false;
                        reverting = decision;
                      }}
                      title={t('RevertTooltip', { path: decision.current_root_folder ?? '' })}
                      aria-label="{t('Revert')} — {decision.media_title}"
                    >
                      <!-- Icon only: one action, on every row, in a table that
                           already overflowed a desktop screen. The name travels
                           in the label rather than in the column. -->
                      <Undo2 size={14} aria-hidden="true" />
                    </button>
                  {/if}
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
            'DecisionCount',
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

  {#if reverting}
    {@const target = reverting}
    <Modal label={t('Revert')} onClose={() => (reverting = null)} maxWidth={520}>
      <div class="modal-header">
        <h2 class="modal-title">{t('Revert')}</h2>
      </div>
      <p>
        {t('ConfirmRevert', {
          title: target.media_title,
          path: target.current_root_folder ?? '',
        })}
      </p>
      <label class="flex items-center gap-2 mt-4">
        <input type="checkbox" bind:checked={revertFiles} />
        {t('ConfirmRevertFiles')}
      </label>
      <div class="flex gap-2 mt-4">
        <button class="btn btn-secondary" onclick={() => (reverting = null)}>{t('Cancel')}</button>
        <button class="btn btn-primary" onclick={() => void revert(target, revertFiles)}>
          {t('Revert')}
        </button>
      </div>
    </Modal>
  {/if}
</div>
