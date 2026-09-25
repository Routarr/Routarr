<script lang="ts">
  import { api } from '../api/client';
  import { createAsync } from '../lib/async.svelte';
  import { createOutcome } from '../lib/outcome.svelte';
  import { askConfirmation } from '../lib/confirm.svelte';
  import { t } from '../lib/i18n.svelte';
  import { Play, Trash2 } from '../lib/icons';
  import type { RuleTestResult, RuleTestRun } from '../api/types';
  import EmptyState from '../components/EmptyState.svelte';
  import ErrorBanner from '../components/ErrorBanner.svelte';
  import Loading from '../components/Loading.svelte';
  import OutcomeBanner from '../components/OutcomeBanner.svelte';
  import TableRegion from '../components/TableRegion.svelte';

  /**
   * Pinned expectations, replayed against the rules as they stand.
   *
   * The rule preview answers "what would this change"; this answers what it must
   * *not* change. Rules are first-match-by-priority, so inserting one rebalances
   * every rule below it, and the routing that quietly moves is the one nobody
   * was watching.
   */
  const cases = createAsync((signal) => api.getRuleTests(signal));

  let run = $state<RuleTestRun | null>(null);
  let busy = $state(false);
  const outcome = createOutcome();

  // Merged by id rather than replacing the list: the run reports on the cases
  // that exist, and the table has to keep showing a case the run never reached.
  const verdicts = $derived(new Map((run?.results ?? []).map((r) => [r.id, r])));

  async function runAll() {
    busy = true;
    // Nothing of the run before stays: its verdicts would read as the answer
    // to this one, whatever this one ends in.
    run = null;
    outcome.clear();
    try {
      run = await api.runRuleTests();
      // The count is the whole answer on a good day, and a case that moved is
      // a failure, read before the table.
      if (run.failed > 0) {
        outcome.fail(t('RuleTestsFailed', { failed: run.failed, total: run.total }));
      } else {
        outcome.succeed(t('RuleTestsPassed', { total: run.total }));
      }
    } catch (err) {
      outcome.fail(err);
    } finally {
      busy = false;
    }
  }

  async function remove(id: string, name: string) {
    if (!(await askConfirmation(t('ConfirmDeleteRuleTest', { name }), 'Delete'))) return;
    try {
      await api.deleteRuleTest(id);
      run = null;
      outcome.succeed(t('RuleTestDeleted'));
      await cases.reload();
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
    <button
      class="btn btn-primary"
      disabled={busy || (cases.data?.length ?? 0) === 0}
      onclick={() => void runAll()}
    >
      <Play size={16} class={busy ? 'spin' : ''} />
      {t('RunRuleTests')}
    </button>
  </div>

  <ErrorBanner
    message={cases.error}
    onDismiss={() => (cases.error = null)}
    onRetry={() => void cases.reload()}
  />
  <OutcomeBanner {outcome} />

  {#if cases.loading}
    <Loading />
  {:else if (cases.data?.length ?? 0) === 0}
    <EmptyState>
      <p><strong>{t('NoRuleTests')}</strong></p>
      <p class="muted">{t('NoRuleTestsHint')}</p>
    </EmptyState>
  {:else}
    <div class="card">
      <TableRegion label={t('RuleTests')}>
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
            {#each cases.data ?? [] as testCase (testCase.id)}
              {@const verdict = verdictOf(testCase.id)}
              <tr>
                <td><strong>{testCase.name}</strong></td>
                <td class="muted">{testCase.source_media_title ?? t('None')}</td>
                <td><span class="badge badge-value">{testCase.expected_category}</span></td>
                <td>
                  {#if !verdict}
                    <span class="muted">{t('NotRunYet')}</span>
                  {:else if verdict.error}
                    <span class="badge badge-danger">{verdict.error}</span>
                  {:else if verdict.passed}
                    <span class="badge badge-success">{t('Passed')}</span>
                    {#if verdict.matched_rule}
                      <span class="muted ms-2">{verdict.matched_rule}</span>
                    {/if}
                  {:else}
                    <!-- Where it lands instead, not just that it moved: that is
                         the question a failure actually raises. -->
                    <span class="badge badge-danger">{t('Failed')}</span>
                    <span class="muted ms-2">
                      {t('RuleTestNowGoesTo', { category: verdict.actual_category ?? t('None') })}
                    </span>
                  {/if}
                </td>
                <td>
                  <button
                    class="btn btn-danger btn-sm"
                    aria-label="{t('Delete')} – {testCase.name}"
                    title={t('Delete')}
                    onclick={() => void remove(testCase.id, testCase.name)}
                  >
                    <Trash2 size={14} />
                  </button>
                </td>
              </tr>
            {/each}
          </tbody>
        </table>
      </TableRegion>
    </div>
  {/if}
</div>
