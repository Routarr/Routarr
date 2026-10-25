<script lang="ts">
  import { Undo2 } from '../lib/icons';
  import { api, type Following } from '../api/client';
  import { dayStart, formatTimestamp, setFields, statusKey, triggerKey } from '../api/format';
  import { handFocus } from '../lib/focus';
  import type { Decision, DecisionFilter, SimulationResult } from '../api/types';
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
  import FilterBar from '../components/FilterBar.svelte';
  import DecisionFilterFields, {
    NO_TITLE_FILTER,
    type TitleFilter,
  } from '../components/DecisionFilterFields.svelte';
  import { endedOnFailure, reportMoves } from '../lib/moves';
  import { access } from '../lib/access.svelte';

  const STATUS_TONE: Record<string, string> = {
    applied: 'badge-success',
    requested: 'badge-info',
    pending: 'badge-warning',
    failed: 'badge-danger',
    skipped: 'badge-info',
  };

  let status = $state('');
  let searchText = $state('');
  let includeSuperseded = $state(false);
  let titles = $state<TitleFilter>({ ...NO_TITLE_FILTER });
  let run = $state('');
  let from = $state('');
  let to = $state('');
  let page = $state(1);

  // What a revert of many takes too: the titles, the simulation and the days.
  const filter = $derived<DecisionFilter>(setFields({ ...titles, search: searchText }));
  const scope = $derived({
    filter,
    simulation_id: run || undefined,
    since: dayStart(from),
    until: dayStart(to, true),
  });
  const narrowed = $derived(
    Object.keys(filter).length > 0 || run !== '' || from !== '' || to !== '',
  );
  // What the panel sets, which the Filters button counts: the search is in sight beside it.
  const panelSet = $derived(
    Object.keys(setFields({ ...titles })).length +
      [status, run, from, to].filter((value) => value !== '').length +
      (includeSuperseded ? 1 : 0),
  );
  const query = $derived({
    ...filter,
    simulation_id: scope.simulation_id,
    since: scope.since,
    until: scope.until,
    status: status || undefined,
    include_superseded: includeSuperseded,
  });

  const history = createAsync(
    (signal) => api.getDecisions({ ...query, page, per_page: 50 }, signal),
    () => [JSON.stringify(query), page],
  );
  /** How many of the moves listed a revert may still undo, which Revert all takes. */
  const revertible = createAsync(
    (signal) => api.countDecisions({ ...query, revertible: true }, signal),
    () => JSON.stringify(query),
  );
  /** The recent simulations, by the time each ran. */
  const runs = createAsync(async (signal) => {
    const jobs = await api.getTasks({ kind: 'simulate', status: 'success', per_page: 20 }, signal);
    return jobs.data.flatMap((job) => {
      const ran = job.result as SimulationResult | null;
      return ran ? [{ id: ran.simulation_id, when: job.started_at }] : [];
    });
  });
  const outcome = createOutcome();

  function clearFilters() {
    searchText = '';
    status = '';
    includeSuperseded = false;
    titles = { ...NO_TITLE_FILTER };
    run = '';
    from = '';
    to = '';
    page = 1;
  }

  const decisions = $derived(history.data?.data ?? []);
  const pagination = $derived(history.data?.pagination);

  // One dialog asks both questions: a second one's Cancel would revert without the files.
  let reverting = $state<Decision | null>(null);
  let revertFiles = $state(false);

  // A reverted row loses its Revert, so the focus goes to a neighbour's, else the table.
  const revertId = (index: number) => `history-revert-${index}`;

  // Stops following, not the revert, whose result the screen opened again reads.
  const leaving = new AbortController();
  $effect(() => () => leaving.abort());

  let revertAllFiles = $state(false);
  let revertingAll = $state(false);
  /** The task of the revert of many being followed, which Stop cancels. */
  let task = $state<string | null>(null);
  const following: Following = { signal: leaving.signal, onProgress: (job) => (task = job.id) };

  /** Undo every move the filters match, asked by the server, which alone counts them. */
  async function revertAll() {
    // Read before the button turns disabled, which drops the focus the question would give back.
    const pressed = document.activeElement as HTMLElement | null;
    revertingAll = true;
    try {
      const report = await answering(
        (answered) => api.revertAllDecisions(scope, revertAllFiles, answered, following),
        'RevertConfirm',
      );
      if (!report) return;
      reportMoves(
        outcome,
        endedOnFailure(report)
          ? t('BatchRevertStopped', {
              applied: report.applied,
              candidates: report.candidates,
              run: report.batches_run,
              planned: report.batches_planned,
            })
          : t('BatchRevertReport', {
              applied: report.applied,
              candidates: report.candidates,
              batches: report.batches_run,
            }),
        report,
        report.stopped_early || report.failed > 0,
      );
      await Promise.all([history.reload(), revertible.reload()]);
    } catch (err) {
      outcome.fail(err);
    } finally {
      revertingAll = false;
      task = null;
      void handFocus(pressed, 'history-table');
    }
  }

  async function stopRevertAll() {
    if (!task) return;
    try {
      await api.cancelTask(task);
    } catch (err) {
      outcome.fail(err);
    }
  }

  async function revert(decision: Decision, moveFiles: boolean) {
    const index = decisions.findIndex((row) => row.id === decision.id);
    reverting = null;
    try {
      // The backend asks before a revert what it asks before an apply.
      // `RevertConfirm`, not `Revert`, which several languages write with their Cancel verb.
      const report = await answering(
        (answered) =>
          api.revertDecisions([decision.id], moveFiles, answered, { signal: leaving.signal }),
        'RevertConfirm',
      );
      if (!report) return;
      await Promise.all([history.reload(), revertible.reload()]);
      void handFocus(revertId(index), revertId(index + 1), revertId(index - 1), 'history-table');
      // Nothing restored is a failure: its reason would read as good news in a success banner.
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
      <h1 class="page-title">{t('History')}</h1>
      <p class="page-subtitle">{t('HistorySubtitle')}</p>
    </div>
  </div>

  <ErrorBanner
    message={history.error}
    onDismiss={() => (history.error = null)}
    onRetry={() => void history.reload()}
  />
  <OutcomeBanner {outcome} />

  <FilterBar
    active={panelSet}
    clearable={narrowed || status !== '' || includeSuperseded}
    onClear={clearFilters}
  >
    {#snippet search()}
      <SearchField
        bind:value={searchText}
        placeholder={t('SearchATitle')}
        label={t('SearchATitle')}
        oninput={() => (page = 1)}
        debounce={200}
      />
    {/snippet}
    <div class="form-group filter-field">
      <label class="form-label" for="history-status">{t('Status')}</label>
      <select
        id="history-status"
        class="form-select"
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
    </div>
    <DecisionFilterFields
      bind:filter={titles}
      fields={['instance', 'category', 'rule']}
      onchange={() => (page = 1)}
    />
    <div class="form-group filter-field">
      <label class="form-label" for="history-run">{t('Simulation')}</label>
      <select
        id="history-run"
        class="form-select"
        value={run}
        onchange={(event) => {
          page = 1;
          run = event.currentTarget.value;
        }}
      >
        <option value="">{t('AllRuns')}</option>
        {#each runs.data ?? [] as ran (ran.id)}
          <option value={ran.id}>
            {t('SimulationOf', { when: formatTimestamp(ran.when, i18n.language) })}
          </option>
        {/each}
      </select>
    </div>
    <div class="form-group filter-field">
      <label class="form-label" for="history-from">{t('DecidedFrom')}</label>
      <input
        id="history-from"
        type="date"
        class="form-input"
        value={from}
        onchange={(event) => {
          page = 1;
          from = event.currentTarget.value;
        }}
      />
    </div>
    <div class="form-group filter-field">
      <label class="form-label" for="history-to">{t('DecidedUntil')}</label>
      <input
        id="history-to"
        type="date"
        class="form-input"
        value={to}
        onchange={(event) => {
          page = 1;
          to = event.currentTarget.value;
        }}
      />
    </div>
    <label class="flex items-center gap-2 text-md filter-check">
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
  </FilterBar>
  <ErrorBanner message={runs.error} onDismiss={() => (runs.error = null)} />

  <ErrorBanner message={revertible.error} onDismiss={() => (revertible.error = null)} />
  {#if revertible.data && !access.readOnly}
    <div class="card">
      <div class="flex flex-wrap items-center justify-between gap-2">
        <label class="flex items-center gap-2 cursor-pointer">
          <input type="checkbox" bind:checked={revertAllFiles} />
          <span>{t('MoveFilesLabel')}</span>
        </label>
        <div class="flex flex-wrap gap-2">
          <button
            class="btn btn-secondary"
            onclick={() => void revertAll()}
            disabled={revertingAll}
            aria-describedby="revert-all-hint"
          >
            <Undo2 size={16} />
            {t(narrowed ? 'RevertMatching' : 'RevertAll', { count: revertible.data })}
          </button>
          {#if revertingAll && task}
            <button class="btn btn-ghost" onclick={() => void stopRevertAll()}>
              {t('StopTask')}
            </button>
          {/if}
        </div>
      </div>
      <p id="revert-all-hint" class="form-hint">{t('RevertAllHint')}</p>
    </div>
  {/if}

  <div class="card">
    <TableRegion label={t('History')} id="history-table">
      <table>
        <caption class="visually-hidden">{t('History')}</caption>
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
                  <!-- Null on a row stored without one, where a guess would read as a fact. -->
                  {#if decision.actor}
                    <span class="text-xs" title={t('TriggeredBy')}
                      ><span class="visually-hidden">{t('TriggeredBy')}</span>{t(
                        triggerKey(decision.actor),
                      )}</span
                    >
                  {/if}
                  <!-- Beside the trigger: "manual" and who answer different questions. -->
                  {#if decision.subject}
                    <span class="text-xs text-muted" title={t('PerformedBy')}
                      ><span class="visually-hidden">{t('PerformedBy')}</span
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
                  {#if decision.revertible && !access.readOnly}
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
                      <!-- Icon only in a wide table: the name travels in the label. -->
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
