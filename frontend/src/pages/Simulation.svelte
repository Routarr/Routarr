<script lang="ts">
  import { Layers, Play, ShieldCheck } from '../lib/icons';
  import { formatBytes } from '../api/format';
  import { ApiError, api } from '../api/client';
  import type { ApplyReport, BatchApplyReport, Decision, SimulationResult } from '../api/types';
  import { describeError } from '../lib/async.svelte';
  import { i18n, t } from '../lib/i18n.svelte';
  import DecisionRow from '../components/DecisionRow.svelte';
  import EmptyState from '../components/EmptyState.svelte';
  import ErrorBanner from '../components/ErrorBanner.svelte';
  import Stat from '../components/Stat.svelte';
  import SuccessBanner from '../components/SuccessBanner.svelte';
  import WarningBanner from '../components/WarningBanner.svelte';
  import { askConfirmation } from '../lib/confirm.svelte';
  import TableRegion from '../components/TableRegion.svelte';
  import { SvelteSet } from 'svelte/reactivity';

  let result = $state<SimulationResult | null>(null);
  const selected = new SvelteSet<string>();
  let moveFiles = $state(false);
  let busy = $state<'run' | 'apply' | null>(null);
  let error = $state<string | null>(null);
  let report = $state<ApplyReport | null>(null);
  let batchReport = $state<BatchApplyReport | null>(null);

  /**
   * Decisions a previous pass left pending.
   *
   * The top bar counts them, so this screen has to show them. Showing only the
   * result of a run made in this browser session lands a user following
   * "12 decisions awaiting review" on "run a simulation" — for the same twelve,
   * already persisted.
   */
  let pending = $state<Decision[] | null>(null);
  // The server's page ceiling. Past it the top bar's count and this list
  // disagree, and the banner below says which one is short.
  const PENDING_PAGE = 200;

  $effect(() => {
    let cancelled = false;
    api
      .getDecisions({ status: 'pending', per_page: PENDING_PAGE })
      .then((page) => {
        if (!cancelled) pending = page.data;
      })
      // A banner, not the empty state: the navigation sent the user here with
      // "12 decisions to review", and a screen saying "run a simulation" over a
      // failed request contradicts it without a word about why.
      .catch((cause: unknown) => {
        if (!cancelled) error = describeError(cause);
      });
    return () => {
      cancelled = true;
    };
  });

  /**
   * Evaluate the library again.
   *
   * `keepReport` matters: applying finishes by refreshing this view, and if that
   * refresh cleared the apply report the user would never learn what happened.
   */
  async function run({ keepReport = false }: { keepReport?: boolean } = {}) {
    busy = 'run';
    error = null;
    if (!keepReport) {
      report = null;
      batchReport = null;
    }
    try {
      const data = await api.runSimulation({ persist: true });
      result = data;
      // Pre-select the actionable proposals only.
      select(
        data.decisions
          .filter((d) => d.action === 'move' && d.status === 'pending')
          .map((d) => d.id),
      );
    } catch (err) {
      error = describeError(err);
    } finally {
      busy = null;
    }
  }

  /** A destination is one folder on one instance; neither alone is unique. */
  const c_key = (c: { instance_id: string; path: string }) => `${c.instance_id}:${c.path}`;

  /**
   * Apply everything this run proposed, not just what is on screen.
   *
   * The list is capped for the payload's sake, so on a large library the
   * selection can only ever cover part of it. Scoping by simulation id rather
   * than by selected rows is what makes "apply all" mean all of them.
   */
  async function applyAll() {
    if (!result) return;
    const proceed = await askConfirmation(
      t('ConfirmApplyAll', { count: result.moves_required }) +
        (moveFiles ? t('ConfirmApplyWithFiles') : '.'),
      'ApplyLabel',
    );
    if (!proceed) return;

    busy = 'apply';
    error = null;
    try {
      batchReport = await api.applyAllDecisions(result.simulation_id, moveFiles, ['batch']);
      await run({ keepReport: true });
    } catch (err) {
      error = describeError(err);
    } finally {
      busy = null;
    }
  }

  /**
   * `answered` carries the guardrails the reader has already looked at.
   *
   * Several can refuse the same apply, and each asks its own question: a
   * blanket yes answered all of them at once, so confirming "there is not
   * enough room" also lifted the batch threshold without showing it. The list
   * grows one name at a time, so nothing is lifted that was not read.
   */
  async function apply(answered: string[] = []): Promise<void> {
    const ids = [...selected];
    if (ids.length === 0) return;

    busy = 'apply';
    error = null;
    try {
      const outcome = await api.applyDecisions(ids, moveFiles, answered);
      report = outcome;
      await run({ keepReport: true });
    } catch (err) {
      // The backend asks for a second, explicit pass past the configured
      // threshold; surface that as a confirmation instead of a raw error.
      if (err instanceof ApiError && err.needsConfirmation && err.confirm) {
        const asking = err.confirm;
        const proceed = await askConfirmation(
          `${err.message}\n\n` +
            t('ConfirmApply', { count: ids.length }) +
            (moveFiles ? t('ConfirmApplyWithFiles') : '.'),
          'ApplyLabel',
        );
        if (proceed) {
          busy = null;
          return apply([...answered, asking]);
        }
      } else {
        error = describeError(err);
      }
    } finally {
      busy = null;
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
  const shown = $derived<Decision[]>(result?.decisions ?? pending ?? []);
  const movable = $derived(shown.filter((d) => d.action === 'move'));
  const allSelected = $derived(movable.length > 0 && movable.every((d) => selected.has(d.id)));
</script>

<div>
  <div class="page-header">
    <div>
      <h1 class="page-title">{t('SimulationTitle')}</h1>
      <p class="page-subtitle">{t('SimulationSubtitle')}</p>
    </div>
    <button class="btn btn-primary" onclick={() => void run()} disabled={busy !== null}>
      <Play size={16} />
      {busy === 'run' ? t('EvaluatingRules') : t('RunSimulation')}
    </button>
  </div>

  <ErrorBanner message={error} onDismiss={() => (error = null)} />

  {#if batchReport}
    {#if batchReport.stopped_early}
      <WarningBanner
        message={t('BatchApplyStopped', {
          applied: batchReport.applied,
          candidates: batchReport.candidates,
          run: batchReport.batches_run,
          planned: batchReport.batches_planned,
        })}
      />
    {:else}
      <SuccessBanner
        message={t('BatchApplyReport', {
          applied: batchReport.applied,
          candidates: batchReport.candidates,
          batches: batchReport.batches_run,
        })}
      />
    {/if}
    {#each batchReport.errors as failure (failure.decision_id)}
      <ErrorBanner message="{failure.media_title}: {failure.message}" />
    {/each}
  {/if}

  {#if report}
    <SuccessBanner
      message={t('ApplyReport', { applied: report.applied, requested: report.requested }) +
        (report.skipped > 0 ? t('ApplyReportSkipped', { count: report.skipped }) : '') +
        '.'}
    />
    {#each report.errors as failure (failure.decision_id)}
      <ErrorBanner message="{failure.media_title}: {failure.message}" />
    {/each}
  {/if}

  {#if !result && shown.length === 0}
    <div class="card"><EmptyState>{t('SimulationEmptyState')}</EmptyState></div>
  {:else}
    <!-- The counters describe a run. Without one there is nothing to count —
         but the decisions themselves are still worth showing. -->
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
        <WarningBanner message={t('SkippedUnmappedWarning', { count: result.skipped_unmapped })} />
      {/if}

      <!-- What the plan weighs, before anything is written. `free_space` is
           synced on every pass and `size_on_disk` sits on every row, and until
           now nothing compared them: a batch that overruns its destination
           fails partway at the Arr and leaves the library half-moved. -->
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
          <div class="card-header"><h2 class="card-title">{t('CapacityTitle')}</h2></div>
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
                        <span class="muted ms-2">{folder.instance_name}</span>
                      {/if}
                    </td>
                    <td>{folder.items}</td>
                    <td>
                      {formatBytes(folder.incoming_bytes, i18n.language)}
                      <!-- Shown rather than dropped: a move between folders on
                           one volume is a rename and costs nothing, and a
                           figure the user cannot see is one they cannot check. -->
                      {#if folder.same_filesystem_bytes > 0}
                        <span class="muted ms-2">
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

      {#if result.returned < result.total_media}
        <WarningBanner
          message={t('TruncatedWarning', { shown: result.returned, total: result.total_media })}
        />
      {/if}
    {/if}

    {#if !result && pending && pending.length >= PENDING_PAGE}
      <WarningBanner message={t('PendingCapped', { count: PENDING_PAGE })} />
    {/if}

    {#if movable.length > 0}
      <div class="card flex items-center justify-between">
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

          <!-- Reaches past the on-screen list: on a large library the table is
               capped, so selecting every visible row is not everything. It
               targets one identified simulation, so it only exists once a run
               has produced one. -->
          {#if result}
            <button
              class="btn btn-secondary"
              onclick={() => void applyAll()}
              disabled={result.moves_required === 0 || busy !== null}
              title={t('ApplyAllHint')}
            >
              <Layers size={16} />
              {t('ApplyAll', { count: result.moves_required })}
            </button>
          {/if}
        </div>
      </div>
    {/if}

    <div class="card">
      <TableRegion label={t('SimulationTitle')}>
        <table>
          <caption class="visually-hidden">{t('SimulationTitle')}</caption>
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
            {#if shown.length === 0}
              <tr><td colspan="8"><EmptyState>{t('NoDecisionGenerated')}</EmptyState></td></tr>
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
    </div>
  {/if}
</div>
