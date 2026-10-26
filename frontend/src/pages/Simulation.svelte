<script lang="ts">
  import Count from '../components/Count.svelte';
  import CardTitle from '../components/CardTitle.svelte';
  import { CheckCircle2, Layers, Play, ShieldCheck } from '../lib/icons';
  import { formatBytes, formatCount, formatTimestamp, setFields } from '../api/format';
  import { ApiError, api, type Following } from '../api/client';
  import type { Decision, DecisionFilter, SimulationResult } from '../api/types';
  import { FIELDS } from '../lib/settings';
  import { createAsync, describeError } from '../lib/async.svelte';
  import { createOutcome } from '../lib/outcome.svelte';
  import { endedOnFailure, reportMoves } from '../lib/moves';
  import { i18n, t } from '../lib/i18n.svelte';
  import { handFocus } from '../lib/focus';
  import { href } from '../lib/router.svelte';
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
  import SearchField from '../components/SearchField.svelte';
  import FilterBar from '../components/FilterBar.svelte';
  import DecisionFilterFields, {
    NO_TITLE_FILTER,
    type TitleFilter,
  } from '../components/DecisionFilterFields.svelte';
  import { SvelteSet } from 'svelte/reactivity';
  import { invalidateStatus } from '../lib/status.svelte';
  import { access } from '../lib/access.svelte';
  import { createExplainer } from '../lib/explain.svelte';
  import ExplanationModal from '../components/ExplanationModal.svelte';

  let result = $state<SimulationResult | null>(null);
  /** When the run on screen started, which nothing else on the screen says. */
  let ranAt = $state<string | null>(null);
  const selected = new SvelteSet<string>();
  let moveFiles = $state(false);
  let busy = $state<'run' | 'apply' | null>(null);
  /** The task of the apply being followed, which Cancel stops. */
  let applying = $state<string | null>(null);
  /** The refresh after an apply, whose failure sits beside the apply's report. */
  let refreshError = $state<string | null>(null);
  const outcome = createOutcome();
  const explainer = createExplainer();

  // The filters narrow the run's proposals, or every pending decision, and an apply of many.
  let titles = $state<TitleFilter>({ ...NO_TITLE_FILTER });
  let searchText = $state('');
  let page = $state(1);
  const filter = $derived<DecisionFilter>(setFields({ ...titles, search: searchText }));
  // What the panel sets, which the Filters button counts: the search is in sight beside it.
  const panelSet = $derived(Object.keys(setFields({ ...titles })).length);
  const filtered = $derived(Object.keys(filter).length > 0);

  /**
   * The moves to review first: a skip waits for its category's folder, and one unmapped category
   * can turn hundreds of titles into skips.
   */
  const VIEWS = [
    { action: 'move', key: 'ViewToMove' },
    { action: 'skip', key: 'ViewSkipped' },
    { action: '', key: 'ViewAll' },
  ] as const;
  type View = (typeof VIEWS)[number]['action'];
  let action = $state<View>('move');
  const narrowing = () => [result?.simulation_id, JSON.stringify(filter), action];

  function show(view: View) {
    action = view;
    page = 1;
  }

  function clearFilters() {
    titles = { ...NO_TITLE_FILTER };
    searchText = '';
    page = 1;
  }

  const PAGE = 50;
  const BATCH_LIMIT = Number(FIELDS.find((field) => field.key === 'batch_limit')?.fallback);
  // Read once and outside the state the list tracks: a preselection past it is refused whole.
  let limit: number | undefined;
  let cap = $state(Infinity);

  /**
   * The settings' batch limit, else its default. A reader is refused the settings: asked before
   * the shell knows who reads, the refusal would otherwise fail the whole list.
   */
  async function batchLimit(signal: AbortSignal): Promise<number> {
    if (access.readOnly) return BATCH_LIMIT;
    try {
      return Number((await api.getSettings(signal)).batch_limit) || BATCH_LIMIT;
    } catch (err) {
      if (err instanceof ApiError && err.status === 403) return BATCH_LIMIT;
      throw err;
    }
  }

  /**
   * The run's proposals, else every pending decision, as the top bar counts them. A run's moves
   * are preselected up to the batch limit in the same update as the rows, or select-all turns
   * clickable while unticked.
   */
  const list = createAsync(
    async (signal) => {
      limit ??= await batchLimit(signal);
      cap = limit;
      const scope = result ? { simulation_id: result.simulation_id } : { status: 'pending' };
      const loaded = await api.getDecisions(
        { ...scope, ...filter, action, page, per_page: PAGE },
        signal,
      );
      const moves = loaded.data.filter((d) => d.action === 'move' && d.status === 'pending');
      select(result ? moves.slice(0, cap).map((d) => d.id) : []);
      return loaded;
    },
    () => [...narrowing(), page],
  );

  /** What each view holds under the filters, the run's proposals or every pending decision. */
  const counts = createAsync(
    async (signal) => {
      const scope = result ? { simulation_id: result.simulation_id } : { status: 'pending' };
      const [move, skip, all] = await Promise.all(
        VIEWS.map((view) =>
          api.countDecisions({ ...scope, ...filter, action: view.action }, signal),
        ),
      );
      return { move, skip, '': all } as Record<View, number>;
    },
    () => [result?.simulation_id, JSON.stringify(filter)],
  );

  /** How many of the run's moves the filters keep, which Apply all applies. */
  const matching = createAsync(async (signal) => {
    if (!result) return 0;
    if (action !== '' && action !== 'move') return 0;
    const scope = { simulation_id: result.simulation_id, status: 'pending', action: 'move' };
    return api.countDecisions({ ...scope, ...filter }, signal);
  }, narrowing);

  /**
   * A newer simulation, by the scheduler or another caller, which replaced the run's proposals.
   * Asked again with each page, so a reader paging a replaced run is told why it emptied.
   */
  const newer = createAsync(
    async (signal) => {
      if (!result) return null;
      const jobs = await api.getTasks({ kind: 'simulate', status: 'success', per_page: 1 }, signal);
      const job = jobs.data[0];
      const latest = job?.result as SimulationResult | null | undefined;
      return job && latest && latest.simulation_id !== result.simulation_id
        ? { result: latest, at: job.started_at }
        : null;
    },
    () => [...narrowing(), page],
  );

  function showNewer() {
    if (!newer.data) return;
    result = newer.data.result;
    ranAt = newer.data.at;
    page = 1;
  }

  /** How far the run being followed has gone, while one is. */
  let progress = $state<{ current: number; total: number } | null>(null);

  /** Stops following, not the run, which the screen opened again finds and follows. */
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
      .getTasks({ kind: 'simulate', status: 'running', per_page: 1 }, leaving.signal)
      .catch(() => null);
    const job = running?.data[0];
    if (!job || busy !== null) return;
    await follow(() => api.followTask<SimulationResult>(job.id, following), job.started_at);
  }

  /**
   * Evaluate the library again. After an apply it runs as a refresh, whose failure shows beside
   * the apply's outcome, never in its place: the moves were made either way.
   */
  async function run({ refresh = false }: { refresh?: boolean } = {}) {
    const at = new Date().toISOString();
    await follow(() => api.runSimulation({ persist: true }, following), at, refresh);
  }

  async function follow(started: () => Promise<SimulationResult>, at: string, refresh = false) {
    const pressed = document.activeElement as HTMLElement | null;
    busy = 'run';
    progress = null;
    refreshError = null;
    if (!refresh) outcome.clear();
    try {
      const summary = await started();
      page = 1;
      result = summary;
      ranAt = at;
    } catch (err) {
      // The screen closed: nothing is left to tell.
      if (leaving.signal.aborted) return;
      if (refresh) refreshError = describeError(err);
      else outcome.fail(err);
    } finally {
      // The shell's counts and its guide read these decisions, whatever the refresh ends in.
      invalidateStatus();
      busy = null;
      progress = null;
      if (!leaving.signal.aborted) void handFocus(pressed);
    }
  }

  /**
   * The apply button is disabled while it writes, and gone when the refresh leaves nothing to
   * apply, so Run takes the focus the button cannot.
   */
  function settleFocus(pressed: HTMLElement | null) {
    if (!leaving.signal.aborted) void handFocus(pressed, 'simulation-run');
  }

  /** Ask the apply being followed to stop before its next move. */
  async function cancelApply() {
    if (!applying) return;
    try {
      await api.cancelTask(applying);
    } catch (err) {
      outcome.fail(err);
    }
  }

  /** A destination is one folder on one instance, and neither alone is unique. */
  const c_key = (c: { instance_id: string; path: string }) => `${c.instance_id}:${c.path}`;

  /**
   * Apply what this run proposed and the filters keep, by simulation id: a page covers only part
   * of a large library. The server asks, as it alone knows a destination is down or full.
   */
  async function applyAll() {
    if (!result) return;
    const simulationId = result.simulation_id;
    const pressed = document.activeElement as HTMLElement | null;

    busy = 'apply';
    try {
      const batch = await answering(
        (answered) =>
          api.applyAllDecisions(simulationId, filter, moveFiles, answered, followingApply),
        'ApplyLabel',
      );
      if (!batch) return;
      // A run a failure cut short says so beside its counts. One stopped says why on its own.
      reportMoves(
        outcome,
        endedOnFailure(batch)
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
      reportMoves(
        outcome,
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

  // An exception to the folder's category: the proposal goes, and the run's counts stay as run.
  async function keep(decision: Decision) {
    try {
      await api.keepTitle(decision.media_id);
      selected.delete(decision.id);
      outcome.succeed(t('KeptHereOutcome', { category: decision.current_category ?? '' }));
      invalidateStatus();
      await reloadProposals();
    } catch (err) {
      outcome.fail(err);
    }
  }

  /** After an exception moved a title out of the proposals, from its row or its explanation. */
  const reloadProposals = () => Promise.all([list.reload(), matching.reload(), counts.reload()]);

  const shown = $derived<Decision[]>(list.data?.data ?? []);
  // True until the list on screen answers, the run's page or the pending decisions.
  const loading = $derived(list.data === null && !list.error);
  const movable = $derived(shown.filter((d) => d.action === 'move'));
  const selectable = $derived(movable.slice(0, cap));
  const allSelected = $derived(
    selectable.length > 0 && selectable.every((d) => selected.has(d.id)),
  );

  // While every destination fits, its table is a press away: it only confirms.
  let capacityShown = $state(false);
  const short = $derived(result?.capacity.filter((c) => !c.fits) ?? []);
  /** Shown while there is something to apply, so no apply is drawn while it cannot act. */
  const applicable = $derived(
    !access.readOnly &&
      (selected.size > 0 || (result !== null && (matching.data ?? 0) > 0) || busy === 'apply'),
  );
  const empty = $derived(
    filtered
      ? 'NoDecisionMatchesFilters'
      : action === 'skip'
        ? 'NoTitleSkipped'
        : action === 'move' && (result || (counts.data?.[''] ?? 0) > 0)
          ? 'NoMoveToReview'
          : result
            ? 'NoDecisionGenerated'
            : 'SimulationEmptyState',
  );
</script>

<div>
  <div class="page-header">
    <div>
      <h1 class="page-title">{t('Simulation')}</h1>
      <p class="page-subtitle">{t('SimulationSubtitle')}</p>
    </div>
    {#if !access.readOnly}
      <div class="flex gap-2">
        <!-- The primary action is the next step: apply once there are moves, else run. -->
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
    {/if}
  </div>

  {#if busy === 'run' && progress && progress.total > 0}
    <div class="card">
      <ProgressBar current={progress.current} total={progress.total} label={t('EvaluatingRules')} />
    </div>
  {/if}

  <ErrorBanner
    message={list.error}
    onDismiss={() => (list.error = null)}
    onRetry={() => void list.reload()}
  />
  <ErrorBanner message={refreshError} onDismiss={() => (refreshError = null)} />
  <ErrorBanner message={explainer.failure} onDismiss={explainer.dismiss} />
  <ErrorBanner
    message={matching.error ?? newer.error ?? counts.error}
    onDismiss={() => {
      matching.error = null;
      newer.error = null;
      counts.error = null;
    }}
  />
  <OutcomeBanner {outcome} />
  <GuideStepBanner step="simulation" />

  {#if newer.data}
    <WarningBanner
      message={t('SimulationReplaced')}
      action={{ label: t('ShowLatestProposals'), run: showNewer }}
    />
  {/if}

  <!-- The figures describe a run, and the moves are worth reviewing without one. -->
  {#if !result && !filtered && (counts.data?.move ?? 0) > 0}
    <WarningBanner message={t('MovesAwaitingReview', { count: counts.data?.move ?? 0 })} />
  {/if}

  {#if result}
    <div class="metrics is-compact">
      {#if ranAt}
        <span class="metric metric-label">
          {t('SimulationOf', { when: formatTimestamp(ranAt, i18n.language) })}
        </span>
      {/if}
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
          screen: t('CategoriesAndFolders'),
        })}
        link={{
          to: '/categories',
          label: t('HelpFixOn', { screen: t('CategoriesAndFolders') }),
        }}
      />
    {/if}

    <!-- A batch past its destination's free space fails partway, the library half-moved. -->
    {#if result.capacity.length > 0}
      {#each short as destination (c_key(destination))}
        <WarningBanner
          message={t('CapacityShortWarning', {
            path: destination.path,
            needed: formatBytes(destination.incoming_bytes, i18n.language),
            free: formatBytes(destination.free_bytes, i18n.language),
          })}
        />
      {/each}

      {#if short.length === 0}
        <p class="page-note flex flex-wrap items-center gap-2">
          <CheckCircle2 size={16} class="text-success" aria-hidden="true" />
          <span>{t('CapacityAllFit')}</span>
          <button
            type="button"
            class="btn btn-secondary btn-sm"
            aria-expanded={capacityShown}
            onclick={() => (capacityShown = !capacityShown)}
          >
            {t('Details')}
          </button>
        </p>
      {/if}

      {#if short.length > 0 || capacityShown}
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
                    <td><Count value={folder.items} /></td>
                    <td>
                      {formatBytes(folder.incoming_bytes, i18n.language)}
                      <!-- A same-volume move is a free rename, shown so the figure checks out. -->
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
  {/if}

  <FilterBar active={panelSet} clearable={filtered} onClear={clearFilters}>
    {#snippet views()}
      <div class="segmented" role="group" aria-label={t('ProposalsShown')}>
        {#each VIEWS as view (view.action)}
          <button
            type="button"
            class="btn btn-secondary"
            aria-pressed={action === view.action}
            onclick={() => show(view.action)}
          >
            {t(view.key)}
            {#if counts.data}
              <span class="badge badge-value muted">
                {formatCount(counts.data[view.action], i18n.language)}
              </span>
            {/if}
          </button>
        {/each}
      </div>
    {/snippet}
    {#snippet search()}
      <SearchField
        bind:value={searchText}
        placeholder={t('SearchATitle')}
        label={t('SearchATitle')}
        oninput={() => (page = 1)}
        debounce={200}
      />
    {/snippet}
    <DecisionFilterFields
      bind:filter={titles}
      fields={['instance', 'type', 'category', 'folder', 'rule']}
      onchange={() => (page = 1)}
    />
  </FilterBar>

  <div class="card">
    {#if action === 'skip' && (counts.data?.skip ?? 0) > 0}
      <p class="card-note">
        {t('SkippedExplained', { count: counts.data?.skip ?? 0 })}
        <a class="text-link" href={href('/categories')}
          >{t('HelpFixOn', { screen: t('CategoriesAndFolders') })}</a
        >
      </p>
    {/if}
    <TableRegion label={t('Simulation')}>
      <table class="table-stacked stack-proposals">
        <caption class="visually-hidden">{t('Simulation')}</caption>
        <thead>
          <tr>
            <th class="w-40">
              {#if !access.readOnly}
                <input
                  type="checkbox"
                  checked={allSelected}
                  disabled={movable.length === 0}
                  onchange={() => select(allSelected ? [] : selectable.map((d) => d.id))}
                  title={t('SelectEveryMove')}
                  aria-label={t('SelectEveryMove')}
                />
              {/if}
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
            <!-- A failed load leaves the word to the banner above. -->
            {#if !list.error}
              <tr>
                <td colspan="8">
                  <EmptyState>{t(empty)}</EmptyState>
                </td>
              </tr>
            {/if}
          {:else}
            {#each shown as decision (decision.id)}
              <DecisionRow
                {decision}
                selected={selected.has(decision.id)}
                onToggle={access.readOnly ? undefined : () => toggle(decision.id)}
                onKeep={access.readOnly ? undefined : () => void keep(decision)}
                {explainer}
              />
            {/each}
          {/if}
        </tbody>
      </table>
    </TableRegion>
    <Pager pagination={list.data?.pagination} bind:page countKey="DecisionCount" />
  </div>

  <!-- Sticky at the foot of the window, beside the rows it applies, and in the page's flow, so
       the last row scrolls clear of it. -->
  {#if applicable}
    <div class="save-bar">
      <div>
        <label class="flex items-center gap-2 cursor-pointer">
          <input type="checkbox" bind:checked={moveFiles} aria-describedby="move-files-hint" />
          <span>{t('MoveFilesLabel')}</span>
        </label>
        <div id="move-files-hint" class="save-bar-scope">{t('MoveFilesHint')}</div>
        {#if movable.length > cap}
          <div class="save-bar-scope">{t('SelectionCapped', { limit: cap })}</div>
        {/if}
      </div>
      <div class="flex flex-wrap gap-2">
        {#if busy === 'apply' && applying}
          <button class="btn btn-ghost" onclick={() => void cancelApply()}>
            {t('StopTask')}
          </button>
        {/if}
        <!-- Reaches past the page, so it shows only once a run produced a simulation. -->
        {#if result && (matching.data ?? 0) > 0}
          <button
            class="btn btn-secondary"
            onclick={() => void applyAll()}
            disabled={busy !== null}
            aria-describedby="apply-all-hint"
          >
            <Layers size={16} />
            {t(filtered ? 'ApplyMatching' : 'ApplyAll', { count: matching.data ?? 0 })}
          </button>
          <span id="apply-all-hint" class="visually-hidden">
            {t(filtered ? 'ApplyMatchingHint' : 'ApplyAllHint')}
          </span>
        {/if}
        {#if selected.size > 0 || busy === 'apply'}
          <button
            class="btn btn-primary"
            onclick={() => void apply()}
            disabled={selected.size === 0 || busy !== null}
          >
            <ShieldCheck size={16} />
            {busy === 'apply' ? t('Applying') : t('ApplySelected', { count: selected.size })}
          </button>
        {/if}
      </div>
    </div>
  {/if}

  {#if explainer.shown}
    <ExplanationModal
      data={explainer.shown}
      onClose={explainer.close}
      onChanged={() => void reloadProposals()}
    />
  {/if}
</div>
