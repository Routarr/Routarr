<script lang="ts">
  import CardTitle from '../components/CardTitle.svelte';
  import { Layers, Play, ShieldCheck } from '../lib/icons';
  import { formatBytes } from '../api/format';
  import { api, type Following } from '../api/client';
  import type { ApplyReport, Decision, SimulationResult, StopReason } from '../api/types';
  import { createAsync, describeError } from '../lib/async.svelte';
  import { createOutcome } from '../lib/outcome.svelte';
  import { i18n, t } from '../lib/i18n.svelte';
  import { handFocus } from '../lib/focus';
  import DecisionRow from '../components/DecisionRow.svelte';
  import Pager from '../components/Pager.svelte';
  import ProgressBar from '../components/ProgressBar.svelte';
  import EmptyState from '../components/EmptyState.svelte';
  import ErrorBanner from '../components/ErrorBanner.svelte';
  import Stat from '../components/Stat.svelte';
  import OutcomeBanner from '../components/OutcomeBanner.svelte';
  import GuideStepBanner from '../components/GuideStepBanner.svelte';
  import WarningBanner from '../components/WarningBanner.svelte';
  import { answering } from '../lib/confirm.svelte';
  import TableRegion from '../components/TableRegion.svelte';
  import TableSkeleton from '../components/TableSkeleton.svelte';
  import { SvelteSet } from 'svelte/reactivity';
  import { invalidateStatus } from '../lib/status.svelte';

  let result = $state<SimulationResult | null>(null);
  const selected = new SvelteSet<string>();
  let moveFiles = $state(false);
  let busy = $state<'run' | 'apply' | null>(null);
  /** The task of the apply being followed, which Cancel stops. */
  let applying = $state<string | null>(null);
  /** The refresh after an apply, whose failure sits beside the apply's report. */
  let refreshError = $state<string | null>(null);
  const outcome = createOutcome();

  /**
   * Decisions a previous pass left pending.
   *
   * The top bar counts them, so this screen has to show them. Showing only the
   * result of a run made in this browser session lands a user following
   * "12 decisions awaiting review" on "run a simulation", for the same twelve,
   * already persisted.
   */
  // The server's page ceiling. Past it the top bar's count and this list
  // disagree, and the banner below says which one is short.
  const PENDING_PAGE = 200;
  // A failure is a banner, not the empty state: the navigation sent the user
  // here with a count of decisions to review, and "run a simulation" over a
  // failed request contradicts it without a word about why.
  const pendingLoad = createAsync((signal) =>
    api.getDecisions({ status: 'pending', per_page: PENDING_PAGE }, signal),
  );
  const pending = $derived<Decision[] | null>(pendingLoad.data?.data ?? null);

  /**
   * The run's own proposals, a page at a time. Its report keeps the counts
   * alone, and `/decisions` lists what it stored by its id.
   */
  let runPage = $state(1);
  // Each page arrives with its moves selected, in the same update as its
  // rows: selected a render later, the select-all box would turn clickable
  // while still unticked.
  const runLoad = createAsync(
    async (signal) => {
      if (!result) return null;
      const page = await api.getDecisions(
        { simulation_id: result.simulation_id, page: runPage, per_page: PENDING_PAGE },
        signal,
      );
      select(
        page.data.filter((d) => d.action === 'move' && d.status === 'pending').map((d) => d.id),
      );
      return page;
    },
    () => [result?.simulation_id, runPage],
  );

  /** How far the run being followed has gone, while one is. */
  let progress = $state<{ current: number; total: number } | null>(null);

  /**
   * Stops following when the screen closes. The run goes on without it, and
   * the screen opened again finds it running and follows it from there.
   */
  const leaving = new AbortController();
  const following: Following = {
    signal: leaving.signal,
    onProgress: (job) => (progress = { current: job.progress_current, total: job.progress_total }),
  };
  const followingApply: Following = {
    signal: leaving.signal,
    onProgress: (job) => (applying = job.id),
  };

  $effect(() => {
    void resume();
    return () => leaving.abort();
  });

  /** A stored run started earlier and still going, by this screen or another caller. */
  async function resume() {
    const running = await api
      .getJobs({ kind: 'simulate', status: 'running', per_page: 1 }, leaving.signal)
      .catch(() => null);
    const job = running?.data[0];
    if (!job || busy !== null) return;
    await follow(() => api.followJob<SimulationResult>(job.id, following));
  }

  /**
   * Evaluate the library again.
   *
   * After an apply it runs as a refresh. The moves were made whatever the
   * refresh ends in, so its failure is shown beside the apply's outcome and
   * never in its place, or the user would not learn what the apply did.
   */
  async function run({ refresh = false }: { refresh?: boolean } = {}) {
    await follow(() => api.runSimulation({ persist: true }, following), refresh);
  }

  async function follow(started: () => Promise<SimulationResult>, refresh = false) {
    const pressed = document.activeElement as HTMLElement | null;
    busy = 'run';
    progress = null;
    refreshError = null;
    if (!refresh) outcome.clear();
    try {
      const summary = await started();
      runPage = 1;
      result = summary;
    } catch (err) {
      // The screen closed: nothing is left to tell.
      if (leaving.signal.aborted) return;
      if (refresh) refreshError = describeError(err);
      else outcome.fail(err);
    } finally {
      // The shell counts the pending and failed decisions, and the guide it
      // draws reads its simulation step from them. After an apply they moved
      // whatever the refresh ends in. Without the revision, the counts wait
      // for the next poll and the step for the next navigation.
      invalidateStatus();
      busy = null;
      progress = null;
      if (!leaving.signal.aborted) void handFocus(pressed);
    }
  }

  /**
   * The apply button turns disabled while it writes, and stays so when the
   * refresh leaves nothing to apply, so Run takes the focus the button cannot.
   */
  function settleFocus(pressed: HTMLElement | null) {
    if (!leaving.signal.aborted) void handFocus(pressed, 'simulation-run');
  }

  const STOPPED: Record<StopReason, string> = {
    dry_run: 'ApplyStoppedByDryRun',
    cancelled: 'ApplyStoppedOnRequest',
    shutdown: 'ApplyStoppedForShutdown',
  };

  /**
   * The outcome of an apply: what it says beside its counts first, then the
   * moves it could not make. A move the Arr is still making, a proposal
   * replaced or a run stopped leaves it unfinished. Nothing made while it is
   * unfinished is a failure, some made is a partial result, and only
   * everything made is a success.
   */
  function reportApply(
    message: string,
    report: Pick<ApplyReport, 'applied' | 'moving' | 'superseded' | 'stopped' | 'errors'>,
    unfinished: boolean,
  ) {
    const said = [
      ...(report.stopped ? [t(STOPPED[report.stopped])] : []),
      ...(report.moving > 0 ? [t('ApplyStillMoving', { count: report.moving })] : []),
      ...(report.superseded > 0 ? [t('ApplyReplaced', { count: report.superseded })] : []),
      ...report.errors.map((failure) => `${failure.media_title}: ${failure.message}`),
    ];
    const made = report.applied + report.moving;
    if (!unfinished && said.length === 0) outcome.succeed(message);
    else if (made === 0) outcome.fail(message, said);
    else outcome.warn(message, said);
  }

  /** Ask the apply being followed to stop before its next move. */
  async function cancelApply() {
    if (!applying) return;
    try {
      await api.cancelJob(applying);
    } catch (err) {
      outcome.fail(err);
    }
  }

  /** A destination is one folder on one instance, and neither alone is unique. */
  const c_key = (c: { instance_id: string; path: string }) => `${c.instance_id}:${c.path}`;

  /**
   * Apply everything this run proposed, not just what is on screen.
   *
   * The list is capped for the payload's sake, so on a large library the
   * selection can only ever cover part of it. Scoping by simulation id rather
   * than by selected rows is what makes "apply all" mean all of them. The
   * question is the server's alone: it names the count, and a destination
   * that is not answering or short of room, which only the server knows.
   */
  async function applyAll() {
    if (!result) return;
    const simulationId = result.simulation_id;
    const pressed = document.activeElement as HTMLElement | null;

    busy = 'apply';
    try {
      const batch = await answering(
        (answered) => api.applyAllDecisions(simulationId, moveFiles, answered, followingApply),
        'ApplyLabel',
      );
      if (!batch) return;
      // A run cut short by a refused slice is unfinished too: the slices after
      // it were never tried. One stopped says why on its own.
      reportApply(
        batch.stopped_early && !batch.stopped
          ? t('BatchApplyStopped', {
              applied: batch.applied,
              candidates: batch.candidates,
              run: batch.batches_run,
              planned: batch.batches_planned,
            })
          : t('BatchApplyReport', {
              applied: batch.applied,
              candidates: batch.candidates,
              batches: batch.batches_run,
            }),
        batch,
        batch.stopped_early || batch.failed > 0,
      );
      await run({ refresh: true });
    } catch (err) {
      outcome.fail(err);
    } finally {
      busy = null;
      applying = null;
      settleFocus(pressed);
    }
  }

  async function apply() {
    const ids = [...selected];
    if (ids.length === 0) return;
    const pressed = document.activeElement as HTMLElement | null;

    busy = 'apply';
    try {
      const done = await answering(
        (answered) => api.applyDecisions(ids, moveFiles, answered, followingApply),
        'ApplyLabel',
        t(moveFiles ? 'ConfirmApplyItemsWithFiles' : 'ConfirmApplyItems', { count: ids.length }),
      );
      if (!done) return;
      reportApply(
        t(done.skipped > 0 ? 'ApplyResultWithSkipped' : 'ApplyResult', {
          applied: done.applied,
          requested: done.requested,
          skipped: done.skipped,
        }),
        done,
        done.failed > 0,
      );
      await run({ refresh: true });
    } catch (err) {
      outcome.fail(err);
    } finally {
      busy = null;
      applying = null;
      settleFocus(pressed);
    }
  }

  function select(ids: Iterable<string>) {
    selected.clear();
    for (const id of ids) selected.add(id);
  }

  function toggle(id: string) {
    if (!selected.delete(id)) selected.add(id);
  }

  // This run if there was one, otherwise what was already waiting. A fresh run
  // replaces them, and supersedes them server-side too.
  const shown = $derived<Decision[]>(result ? (runLoad.data?.data ?? []) : (pending ?? []));
  // The list on screen has not answered yet: the run's page, or the pending
  // decisions no run has replaced.
  const loading = $derived(
    result ? runLoad.data === null && !runLoad.error : pending === null && !pendingLoad.error,
  );
  const movable = $derived(shown.filter((d) => d.action === 'move'));
  const allSelected = $derived(movable.length > 0 && movable.every((d) => selected.has(d.id)));
</script>

<div>
  <div class="page-header">
    <div>
      <h1 class="page-title">{t('Simulation')}</h1>
      <p class="page-subtitle">{t('SimulationSubtitle')}</p>
    </div>
    <div class="flex gap-2">
      <!-- The screen's one primary action is its next step: applying, once
           there are moves to apply, else running. -->
      <button
        id="simulation-run"
        class="btn {movable.length > 0 ? 'btn-secondary' : 'btn-primary'}"
        onclick={() => void run()}
        disabled={busy !== null}
      >
        <Play size={16} />
        {busy === 'run' ? t('EvaluatingRules') : t('RunSimulation')}
      </button>
    </div>
  </div>

  {#if busy === 'run' && progress && progress.total > 0}
    <div class="card">
      <ProgressBar current={progress.current} total={progress.total} label={t('EvaluatingRules')} />
    </div>
  {/if}

  <ErrorBanner
    message={pendingLoad.error}
    onDismiss={() => (pendingLoad.error = null)}
    onRetry={() => void pendingLoad.reload()}
  />
  <ErrorBanner message={refreshError} onDismiss={() => (refreshError = null)} />
  <ErrorBanner
    message={runLoad.error}
    onDismiss={() => (runLoad.error = null)}
    onRetry={() => void runLoad.reload()}
  />
  <OutcomeBanner {outcome} />
  <GuideStepBanner step="simulation" />

  <!-- The counters describe a run. Without one there is nothing to count, but
       the decisions themselves are still worth showing. -->
  {#if !result && shown.length > 0}
    <WarningBanner message={t('DecisionsAwaitingReview', { count: shown.length })} />
  {/if}

  {#if result}
    <div class="metrics">
      <Stat label={t('Evaluated')} value={result.total_media} />
      <Stat label={t('MovesRequired')} value={result.moves_required} tone="warning" />
      <Stat label={t('AlreadyCorrect')} value={result.already_correct} tone="success" />
      <Stat label={t('NoRuleMatched')} value={result.no_category_match} />
      <Stat label={t('UnmappedCategory')} value={result.skipped_unmapped} tone="danger" />
      <Stat label={t('OverridesLabel')} value={result.overrides_applied} />
    </div>

    {#if result.skipped_unmapped > 0}
      <WarningBanner
        message={t('SkippedUnmappedWarning', {
          count: result.skipped_unmapped,
          screen: t('RootFolders'),
        })}
      />
    {/if}

    <!-- What the plan weighs, before anything is written. `free_space` is
           synced on every pass and `size_on_disk` sits on every row, and the
           plan compares the two: a batch that overruns its destination fails
           partway at the Arr and leaves the library half-moved. -->
    {#if result.capacity.length > 0}
      {#each result.capacity.filter((c) => !c.fits) as short (c_key(short))}
        <WarningBanner
          message={t('CapacityShortWarning', {
            path: short.path,
            needed: formatBytes(short.incoming_bytes, i18n.language),
            free: formatBytes(short.free_bytes, i18n.language),
          })}
        />
      {/each}

      <div class="card">
        <div class="card-header">
          <CardTitle card="CapacityTitle">{t('CapacityTitle')}</CardTitle>
        </div>
        <TableRegion label={t('CapacityTitle')}>
          <table>
            <caption class="visually-hidden">{t('CapacityTitle')}</caption>
            <thead>
              <tr>
                <th>{t('Destination')}</th>
                <th>{t('Items')}</th>
                <th>{t('CapacityIncoming')}</th>
                <th>{t('CapacityFree')}</th>
              </tr>
            </thead>
            <tbody>
              {#each result.capacity as folder (c_key(folder))}
                <tr>
                  <td>
                    <span class="mono">{folder.path}</span>
                    {#if folder.instance_name}
                      <span class="text-muted ms-2">{folder.instance_name}</span>
                    {/if}
                  </td>
                  <td>{folder.items}</td>
                  <td>
                    {formatBytes(folder.incoming_bytes, i18n.language)}
                    <!-- Shown rather than dropped: a move between folders on
                           one volume is a rename and costs nothing, and a
                           figure the user cannot see is one they cannot check. -->
                    {#if folder.same_filesystem_bytes > 0}
                      <span class="text-muted ms-2">
                        {t('CapacitySameVolume', {
                          size: formatBytes(folder.same_filesystem_bytes, i18n.language),
                        })}
                      </span>
                    {/if}
                  </td>
                  <td class={folder.fits ? '' : 'text-danger'}>
                    {formatBytes(folder.free_bytes, i18n.language)}
                  </td>
                </tr>
              {/each}
            </tbody>
          </table>
        </TableRegion>
      </div>
    {/if}
  {/if}

  {#if !result && pending && pending.length >= PENDING_PAGE}
    <WarningBanner message={t('PendingCapped', { count: PENDING_PAGE })} />
  {/if}

  {#if movable.length > 0}
    <div class="card">
      <div class="flex items-center justify-between">
        <label class="flex items-center gap-2 cursor-pointer">
          <input type="checkbox" bind:checked={moveFiles} />
          <span>{t('MoveFilesLabel')}</span>
        </label>

        <div class="flex gap-2">
          <button
            class="btn btn-primary"
            onclick={() => void apply()}
            disabled={selected.size === 0 || busy !== null}
          >
            <ShieldCheck size={16} />
            {busy === 'apply' ? t('Applying') : t('ApplySelected', { count: selected.size })}
          </button>

          <!-- Reaches past the page on screen: selecting every visible row is
               not everything. It targets one identified simulation, so it only
               exists once a run has produced one. -->
          {#if result}
            <button
              class="btn btn-secondary"
              onclick={() => void applyAll()}
              disabled={result.moves_required === 0 || busy !== null}
              aria-describedby="apply-all-hint"
            >
              <Layers size={16} />
              {t('ApplyAll', { count: result.moves_required })}
            </button>
          {/if}
          {#if busy === 'apply' && applying}
            <button class="btn btn-ghost" onclick={() => void cancelApply()}>
              {t('StopTask')}
            </button>
          {/if}
        </div>
      </div>
      {#if result}
        <p id="apply-all-hint" class="form-hint">{t('ApplyAllHint')}</p>
      {/if}
    </div>
  {/if}

  <div class="card">
    <TableRegion label={t('Simulation')}>
      <table>
        <caption class="visually-hidden">{t('Simulation')}</caption>
        <thead>
          <tr>
            <th class="w-40">
              <input
                type="checkbox"
                checked={allSelected}
                disabled={movable.length === 0}
                onchange={() => select(allSelected ? [] : movable.map((d) => d.id))}
                title={t('SelectEveryMove')}
                aria-label={t('SelectEveryMove')}
              />
            </th>
            <th>{t('Media')}</th>
            <th>{t('Current')}</th>
            <th><span class="visually-hidden">{t('Action')}</span></th>
            <th>{t('Target')}</th>
            <th>{t('Rule')}</th>
            <th class="w-90">{t('Confidence')}</th>
            <th>{t('Why')}</th>
          </tr>
        </thead>
        <tbody>
          {#if loading}
            <TableSkeleton columns={8} />
          {:else if shown.length === 0}
            <!-- Nothing pending and nothing run says to run, a run that
                   proposed nothing says so. A list that failed to load says
                   neither: its banner above does. -->
            {#if !(result ? runLoad.error : pendingLoad.error)}
              <tr>
                <td colspan="8">
                  <EmptyState>
                    {t(result ? 'NoDecisionGenerated' : 'SimulationEmptyState')}
                  </EmptyState>
                </td>
              </tr>
            {/if}
          {:else}
            {#each shown as decision (decision.id)}
              <DecisionRow
                {decision}
                selected={selected.has(decision.id)}
                onToggle={() => toggle(decision.id)}
              />
            {/each}
          {/if}
        </tbody>
      </table>
    </TableRegion>
    {#if result}
      <Pager pagination={runLoad.data?.pagination} bind:page={runPage} countKey="DecisionCount" />
    {/if}
  </div>
</div>
