<script lang="ts">
  import { Undo2 } from '../lib/icons';
  import { api } from '../api/client';
  import { formatTimestamp, statusKey, triggerKey } from '../api/format';
  import { handFocus } from '../lib/focus';
  import type { Decision } from '../api/types';
  import { createAsync } from '../lib/async.svelte';
  import { answering } from '../lib/confirm.svelte';
  import { createOutcome } from '../lib/outcome.svelte';
  import { i18n, t } from '../lib/i18n.svelte';
  import NoValue from '../components/NoValue.svelte';
  import Confidence from '../components/Confidence.svelte';
  import EmptyState from '../components/EmptyState.svelte';
  import ErrorBanner from '../components/ErrorBanner.svelte';
  import Modal from '../components/Modal.svelte';
  import OutcomeBanner from '../components/OutcomeBanner.svelte';
  import TableSkeleton from '../components/TableSkeleton.svelte';
  import TableRegion from '../components/TableRegion.svelte';
  import Pager from '../components/Pager.svelte';
  import SearchField from '../components/SearchField.svelte';

  const STATUS_TONE: Record<string, string> = {
    applied: 'badge-success',
    requested: 'badge-info',
    pending: 'badge-warning',
    failed: 'badge-danger',
    skipped: 'badge-info',
  };

  let status = $state('');
  let search = $state('');
  let includeSuperseded = $state(false);
  let page = $state(1);

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
  const outcome = createOutcome();

  const decisions = $derived(history.data?.data ?? []);
  const pagination = $derived(history.data?.pagination);

  // One dialog asks both questions. Chaining two ("revert?", then "move the
  // files too?") gives the second no context, and cancelling it would mean
  // "revert without moving files" rather than "stop": a Cancel button that does
  // not cancel. Here Cancel means cancel.
  let reverting = $state<Decision | null>(null);
  let revertFiles = $state(false);

  // A reverted row stays and loses its Revert: the focus goes to the Revert of
  // the row after it, else the one before, else the table.
  const revertId = (index: number) => `history-revert-${index}`;

  // Stops following a revert when the screen closes. The revert goes on, and
  // the screen opened again reads where it went.
  const leaving = new AbortController();
  $effect(() => () => leaving.abort());

  async function revert(decision: Decision, moveFiles: boolean) {
    const index = decisions.findIndex((row) => row.id === decision.id);
    reverting = null;
    try {
      // A revert writes into the folder the move came from, and the backend
      // asks there what it asks before an apply.
      // `RevertConfirm`, not `Revert`: a button beside Cancel, which several
      // languages would otherwise start with their Cancel verb.
      const report = await answering(
        (answered) =>
          api.revertDecisions([decision.id], moveFiles, answered, { signal: leaving.signal }),
        'RevertConfirm',
      );
      if (!report) return;
      await history.reload();
      void handFocus(revertId(index), revertId(index + 1), revertId(index - 1), 'history-table');
      // A revert that restored nothing did not do what was asked: under the
      // success banner its reason would read as good news.
      if (report.applied > 0) outcome.succeed(t('RevertResult', { count: report.applied }));
      else
        outcome.fail(
          report.errors[0]
            ? t('NothingRevertedBecause', { reason: report.errors[0].message })
            : t('NothingReverted'),
        );
    } catch (err) {
      outcome.fail(err);
    }
  }
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
  <OutcomeBanner {outcome} />

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
      <option value="requested">{t('StatusRequested')}</option>
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
    <TableRegion label={t('AuditHistory')} id="history-table">
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
            <th class="w-70"><span class="visually-hidden">{t('Actions')}</span></th>
          </tr>
        </thead>
        <tbody>
          {#if history.loading && decisions.length === 0}
            <TableSkeleton columns={8} />
          {:else if decisions.length === 0 && !history.error}
            <tr><td colspan="8"><EmptyState>{t('NoDecisionRecorded')}</EmptyState></td></tr>
          {:else}
            {#each decisions as decision, index (decision.id)}
              <tr class:row-muted={decision.superseded}>
                <td class="cell-timestamp" title={decision.decided_at}>
                  {formatTimestamp(decision.decided_at, i18n.language)}
                  <!-- Null on a row stored without one, and a guess there
                       would read as a fact. -->
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
                  <strong class="cell-title">{decision.media_title}</strong>
                  <div class="text-muted text-sm">{decision.instance_name}</div>
                </td>
                <td><span class="badge badge-value">{decision.target_category}</span></td>
                <td class="mono text-sm">
                  <span class="cell-path">
                    {#if decision.target_root_folder}
                      <bdi>{decision.target_root_folder}</bdi>
                    {:else}
                      <NoValue />
                    {/if}
                  </span>
                </td>
                <td>{decision.matched_rule_name ?? t('DefaultCategoryFallback')}</td>
                <td><Confidence value={decision.confidence} /></td>
                <td>
                  <span class="badge {STATUS_TONE[decision.status] ?? 'badge-info'}">
                    {t(statusKey(decision.status))}
                  </span>
                  {#if decision.status === 'requested'}
                    <div class="text-muted text-xs">{t('RequestedHint')}</div>
                  {/if}
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
                  {#if decision.revertible}
                    <button
                      id={revertId(index)}
                      class="btn btn-secondary btn-sm"
                      onclick={() => {
                        revertFiles = false;
                        reverting = decision;
                      }}
                      title={t('RevertTooltip', { path: decision.current_root_folder ?? '' })}
                      aria-label="{t('Revert')} – {decision.media_title}"
                    >
                      <!-- Icon only: one action, on every row, in a table that
                           already fills a desktop screen. The name travels in
                           the label rather than in the column. -->
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

    <Pager {pagination} bind:page countKey="DecisionCount" />
  </div>

  {#if reverting}
    {@const target = reverting}
    <Modal label={t('Revert')} onClose={() => (reverting = null)} maxWidth={520}>
      <div class="modal-header">
        <h2 class="modal-title">{t('Revert')}</h2>
        <button
          class="btn btn-secondary btn-sm"
          onclick={() => (reverting = null)}
          aria-label={t('Dismiss')}
          title={t('Dismiss')}
        >
          ✕
        </button>
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
      <div class="dialog-actions">
        <button class="btn btn-secondary" onclick={() => (reverting = null)}>{t('Cancel')}</button>
        <div class="flex flex-wrap gap-2">
          <button class="btn btn-primary" onclick={() => void revert(target, revertFiles)}>
            {t('RevertConfirm')}
          </button>
        </div>
      </div>
    </Modal>
  {/if}
</div>
