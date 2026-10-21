<script lang="ts">
  import { api } from '../api/client';
  import { createAsync } from '../lib/async.svelte';
  import { createOutcome } from '../lib/outcome.svelte';
  import { askConfirmation } from '../lib/confirm.svelte';
  import { handFocus } from '../lib/focus';
  import { i18n, t } from '../lib/i18n.svelte';
  import { formatTimestamp } from '../api/format';
  import { invalidateStatus } from '../lib/status.svelte';
  import { access } from '../lib/access.svelte';
  import { Play, Trash2 } from '../lib/icons';
  import type { RuleTestResult, RuleTestRun } from '../api/types';
  import NoValue from '../components/NoValue.svelte';
  import EmptyState from '../components/EmptyState.svelte';
  import ErrorBanner from '../components/ErrorBanner.svelte';
  import OutcomeBanner from '../components/OutcomeBanner.svelte';
  import TableSkeleton from '../components/TableSkeleton.svelte';
  import TableRegion from '../components/TableRegion.svelte';
  import Pager from '../components/Pager.svelte';

  /**
   * Pinned expectations, replayed against the rules as they stand: the first match by priority
   * wins, so one new rule can quietly move what every rule below it routed.
   */
  let page = $state(1);
  const cases = createAsync(
    (signal) => api.getRuleTests({ page, per_page: 50 }, signal),
    () => [page],
  );
  const listed = $derived(cases.data?.data ?? []);
  const total = $derived(cases.data?.pagination.total ?? 0);

  let run = $state<RuleTestRun | null>(null);
  let busy = $state(false);
  const outcome = createOutcome();

  // Merged by id, so the table keeps showing a case the run never reached.
  const verdicts = $derived(new Map((run?.results ?? []).map((r) => [r.id, r])));

  async function runAll() {
    const pressed = document.activeElement as HTMLElement | null;
    busy = true;
    // Cleared first: the last run's verdicts would read as the answer to this one.
    run = null;
    outcome.clear();
    try {
      run = await api.runRuleTests();
      invalidateStatus();
      void cases.reload();
      // A case that moved is a failure, and the count says so before the table.
      if (run.failed > 0) {
        outcome.fail(t('RuleTestsFailed', { failed: run.failed, total: run.total }));
      } else {
        outcome.succeed(t('RuleTestsPassed', { total: run.total }));
      }
    } catch (err) {
      outcome.fail(err);
    } finally {
      busy = false;
      void handFocus(pressed);
    }
  }

  const deleteId = (index: number) => `rule-tests-delete-${index}`;

  // A deletion takes the pressed Delete with it, so the focus goes to the nearest row left.
  async function remove(id: string, name: string, index: number) {
    if (!(await askConfirmation(t('ConfirmDeleteRuleTest', { name }), 'Delete'))) return;
    try {
      await api.deleteRuleTest(id);
      run = null;
      invalidateStatus();
      outcome.succeed(t('RuleTestDeleted'));
      await cases.reload();
      // The last case of the last page leaves the page before it to show.
      if (listed.length === 0 && page > 1) page -= 1;
      void handFocus(deleteId(index), deleteId(index - 1), 'rule-tests-table');
    } catch (err) {
      outcome.fail(err);
    }
  }

  function verdictOf(id: string): RuleTestResult | undefined {
    return verdicts.get(id);
  }
</script>

<div>
  <div class="page-header">
    <div>
      <h1 class="page-title">{t('RuleTests')}</h1>
      <p class="page-subtitle">{t('RuleTestsSubtitle')}</p>
    </div>
    {#if !access.readOnly}
      <div class="flex gap-2">
        <button
          class="btn btn-primary"
          disabled={busy || total === 0}
          onclick={() => void runAll()}
        >
          <Play size={16} class={busy ? 'spin' : ''} />
          {busy ? t('RunningRuleTests') : t('RunRuleTests')}
        </button>
      </div>
    {/if}
  </div>

  <ErrorBanner
    message={cases.error}
    onDismiss={() => (cases.error = null)}
    onRetry={() => void cases.reload()}
  />
  <OutcomeBanner {outcome} />

  <div class="card">
    <TableRegion id="rule-tests-table" label={t('RuleTests')}>
      <table>
        <caption class="visually-hidden">{t('RuleTests')}</caption>
        <thead>
          <tr>
            <th>{t('Name')}</th>
            <th>{t('PinnedFrom')}</th>
            <th>{t('ExpectedCategory')}</th>
            <th>{t('Result')}</th>
            <th><span class="visually-hidden">{t('Actions')}</span></th>
          </tr>
        </thead>
        <tbody>
          <!-- The first load only: a reload that swapped the rows for a skeleton
               would take the focus with them. -->
          {#if cases.loading && cases.data === null}
            <TableSkeleton columns={5} />
          {:else if total === 0 && !cases.error}
            <tr>
              <td colspan="5">
                <EmptyState>{t('NoRuleTests')}</EmptyState>
              </td>
            </tr>
          {:else}
            {#each listed as testCase, index (testCase.id)}
              {@const verdict = verdictOf(testCase.id)}
              <tr>
                <td><strong>{testCase.name}</strong></td>
                <td class="text-muted">
                  {#if testCase.source_media_title}{testCase.source_media_title}{:else}<NoValue
                    />{/if}
                </td>
                <td><span class="badge badge-value">{testCase.expected_category}</span></td>
                <td>
                  {#if !verdict && testCase.last_passed === null}
                    <span class="text-muted">{t('NotRunYet')}</span>
                  {:else if !verdict}
                    <!-- The last run's verdict, kept by the server: after a sync or a pin. -->
                    <span class="badge {testCase.last_passed ? 'badge-success' : 'badge-danger'}">
                      {t(testCase.last_passed ? 'Passed' : 'Failed')}
                    </span>
                    {#if testCase.last_checked_at}
                      <span class="text-muted ms-2">
                        {t('RuleTestCheckedAt', {
                          when: formatTimestamp(testCase.last_checked_at, i18n.language),
                        })}
                      </span>
                    {/if}
                  {:else if verdict.error}
                    <span class="badge badge-danger">{verdict.error}</span>
                  {:else if verdict.passed}
                    <span class="badge badge-success">{t('Passed')}</span>
                    {#if verdict.matched_rule}
                      <span class="text-muted ms-2">{verdict.matched_rule}</span>
                    {/if}
                  {:else}
                    <!-- Where it lands instead: the question a failure raises. -->
                    <span class="badge badge-danger">{t('Failed')}</span>
                    <span class="text-muted ms-2">
                      {t('RuleTestNowGoesTo', {
                        category: verdict.actual_category ?? t('NoneSpoken'),
                      })}
                    </span>
                  {/if}
                </td>
                <td>
                  {#if !access.readOnly}
                    <button
                      id={deleteId(index)}
                      class="btn btn-danger btn-sm"
                      aria-label="{t('Delete')} – {testCase.name}"
                      title={t('Delete')}
                      onclick={() => void remove(testCase.id, testCase.name, index)}
                    >
                      <Trash2 size={14} />
                    </button>
                  {/if}
                </td>
              </tr>
            {/each}
          {/if}
        </tbody>
      </table>
    </TableRegion>
    <Pager pagination={cases.data?.pagination} bind:page countKey="RuleTestCount" />
  </div>
</div>
