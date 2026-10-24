<script lang="ts">
  import Count from '../components/Count.svelte';
  import CardTitle from '../components/CardTitle.svelte';
  import { Layers, Play, ShieldCheck } from '../lib/icons';
  import { formatBytes, setFields } from '../api/format';
  import { ApiError, api, type Following } from '../api/client';
  import type { Decision, DecisionFilter, SimulationResult } from '../api/types';
  import { FIELDS } from '../lib/settings';
  import { createAsync, describeError } from '../lib/async.svelte';
  import { createOutcome } from '../lib/outcome.svelte';
  import { endedOnFailure, reportMoves } from '../lib/moves';
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
  import SearchField from '../components/SearchField.svelte';
  import FilterBar from '../components/FilterBar.svelte';
  import DecisionFilterFields, {
    NO_TITLE_FILTER,
    type TitleFilter,
  } from '../components/DecisionFilterFields.svelte';
  import { SvelteSet } from 'svelte/reactivity';
  import { invalidateStatus } from '../lib/status.svelte';
  import { access } from '../lib/access.svelte';

  let result = $state<SimulationResult | null>(null);
  const selected = new SvelteSet<string>();
  let moveFiles = $state(false);
  let busy = $state<'run' | 'apply' | null>(null);
  /** The task of the apply being followed, which Cancel stops. */
  let applying = $state<string | null>(null);
  /** The refresh after an apply, whose failure sits beside the apply's report. */
  let refreshError = $state<string | null>(null);
  const outcome = createOutcome();

  // The filters narrow the run's proposals, or every pending decision, and an apply of many.
  let titles = $state<TitleFilter>({ ...NO_TITLE_FILTER });
  let searchText = $state('');
  let action = $state('');
  let page = $state(1);
  const filter = $derived<DecisionFilter>(setFields({ ...titles, search: searchText }));
  // What the panel sets, which the Filters button counts: the search is in sight beside it.
  const panelSet = $derived(Object.keys(setFields({ ...titles })).length + (action === '' ? 0 : 1));
  const filtered = $derived(Object.keys(filter).length > 0 || action !== '');
  const narrowing = () => [result?.simulation_id, JSON.stringify(filter), action];

  function clearFilters() {
    titles = { ...NO_TITLE_FILTER };
    searchText = '';
    action = '';
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
      const latest = jobs.data[0]?.result as SimulationResult | null | undefined;
      return latest && latest.simulation_id !== result.simulation_id ? latest : null;
    },
    () => [...narrowing(), page],
  );

  function showNewer() {
    if (!newer.data) return;
    result = newer.data;
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
    await follow(() => api.followTask<SimulationResult>(job.id, following));
  }

  /**
   * Evaluate the library again. After an apply it runs as a refresh, whose failure shows beside
   * the apply's outcome, never in its place: the moves were made either way.
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
      page = 1;
      result = summary;
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
   * The apply button is disabled while it writes, and stays so when the refresh leaves nothing to
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
      await Promise.all([list.reload(), matching.reload()]);
    } catch (err) {
      outcome.fail(err);
    }
  }

  const shown = $derived<Decision[]>(list.data?.data ?? []);
  // True until the list on screen answers, the run's page or the pending decisions.
  const loading = $derived(list.data === null && !list.error);
  const movable = $derived(shown.filter((d) => d.action === 'move'));
  const selectable = $derived(movable.slice(0, cap));
  const allSelected = $derived(
    selectable.length > 0 && selectable.every((d) => selected.has(d.id)),
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
  <ErrorBanner
    message={matching.error ?? newer.error}
    onDismiss={() => {
      matching.error = null;
      newer.error = null;
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

  <!-- The counters describe a run, and the decisions are worth showing without one. -->
  {#if !result && list.data && list.data.pagination.total > 0 && !filtered}
    <WarningBanner message={t('DecisionsAwaitingReview', { count: list.data.pagination.total })} />
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
          screen: t('CategoriesAndFolders'),
        })}
      />
    {/if}

    <!-- A batch past its destination's free space fails partway, the library half-moved. -->
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

  <FilterBar active={panelSet} clearable={filtered} onClear={clearFilters}>
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
    <div class="form-group filter-field">
      <label class="form-label" for="simulation-action">{t('Action')}</label>
      <select
        id="simulation-action"
        class="form-select"
        value={action}
        onchange={(event) => {
          page = 1;
          action = event.currentTarget.value;
        }}
      >
        <option value="">{t('AllActions')}</option>
        <option value="move">{t('ActionMove')}</option>
        <option value="skip">{t('ActionSkip')}</option>
      </select>
    </div>
  </FilterBar>

  {#if movable.length > 0 && !access.readOnly}
    <div class="card">
      <div class="flex flex-wrap items-center justify-between gap-2">
        <label class="flex items-center gap-2 cursor-pointer">
          <input type="checkbox" bind:checked={moveFiles} />
          <span>{t('MoveFilesLabel')}</span>
        </label>

        <div class="flex flex-wrap gap-2">
          <button
            class="btn btn-primary"
            onclick={() => void apply()}
            disabled={selected.size === 0 || busy !== null}
          >
            <ShieldCheck size={16} />
            {busy === 'apply' ? t('Applying') : t('ApplySelected', { count: selected.size })}
          </button>

          <!-- Reaches past the page, so it shows only once a run produced a simulation. -->
          {#if result}
            <button
              class="btn btn-secondary"
              onclick={() => void applyAll()}
              disabled={!matching.data || busy !== null}
              aria-describedby="apply-all-hint"
            >
              <Layers size={16} />
              {t(filtered ? 'ApplyMatching' : 'ApplyAll', { count: matching.data ?? 0 })}
            </button>
          {/if}
          {#if busy === 'apply' && applying}
            <button class="btn btn-ghost" onclick={() => void cancelApply()}>
              {t('StopTask')}
            </button>
          {/if}
        </div>
      </div>
      {#if movable.length > cap}
        <p class="form-hint">{t('SelectionCapped', { limit: cap })}</p>
      {/if}
      {#if result}
        <p id="apply-all-hint" class="form-hint">
          {t(filtered ? 'ApplyMatchingHint' : 'ApplyAllHint')}
        </p>
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
                  <EmptyState>
                    {t(
                      filtered
                        ? 'NoDecisionMatchesFilters'
                        : result
                          ? 'NoDecisionGenerated'
                          : 'SimulationEmptyState',
                    )}
                  </EmptyState>
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
              />
            {/each}
          {/if}
        </tbody>
      </table>
    </TableRegion>
    <Pager pagination={list.data?.pagination} bind:page countKey="DecisionCount" />
  </div>
</div>
