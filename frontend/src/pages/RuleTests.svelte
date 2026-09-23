<script lang="ts">
  import { api } from '../api/client';
  import { createAsync, describeError } from '../lib/async.svelte';
  import { askConfirmation } from '../lib/confirm.svelte';
  import { t } from '../lib/i18n.svelte';
  import { CheckCircle2, Play, Trash2, XCircle } from '../lib/icons';
  import type { RuleTestResult, RuleTestRun } from '../api/types';
  import EmptyState from '../components/EmptyState.svelte';
  import ErrorBanner from '../components/ErrorBanner.svelte';
  import Loading from '../components/Loading.svelte';
  import SuccessBanner from '../components/SuccessBanner.svelte';
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

  let outcome = $state<RuleTestRun | null>(null);
  let busy = $state(false);
  let notice = $state<string | null>(null);

  // Merged by id rather than replacing the list: the run reports on the cases
  // that exist, and the table has to keep showing a case the run never reached.
  const verdicts = $derived(new Map((outcome?.results ?? []).map((r) => [r.id, r])));

  async function runAll() {
    busy = true;
    cases.error = null;
    notice = null;
    try {
      outcome = await api.runRuleTests();
    } catch (err) {
      cases.error = describeError(err);
    } finally {
      busy = false;
    }
  }

  async function remove(id: string, name: string) {
    if (!(await askConfirmation(t('ConfirmDeleteRuleTest', { name }), 'Delete'))) return;
    try {
      await api.deleteRuleTest(id);
      outcome = null;
      notice = t('RuleTestDeleted');
      await cases.reload();
    } catch (err) {
      cases.error = describeError(err);
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
  <SuccessBanner message={notice} />

  {#if outcome}
    <!-- The count first, because "12 of 13" is the whole answer on a good day
         and the table is only needed on a bad one. -->
    <!-- Announced like the shared banners: a failed case is an alert, a clean
         run a status — a plain div reached no reader either way. -->
    <div
      class="banner {outcome.failed > 0 ? 'banner-danger' : 'banner-success'}"
      role={outcome.failed > 0 ? 'alert' : 'status'}
    >
      {#if outcome.failed > 0}
        <XCircle size={16} />
        {t('RuleTestsFailed', { failed: outcome.failed, total: outcome.total })}
      {:else}
        <CheckCircle2 size={16} />
        {t('RuleTestsPassed', { total: outcome.total })}
      {/if}
    </div>
  {/if}

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
                <td class="muted">{testCase.source_media_title ?? '—'}</td>
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
                      {t('RuleTestNowGoesTo', { category: verdict.actual_category ?? '—' })}
                    </span>
                  {/if}
                </td>
                <td>
                  <button
                    class="btn btn-danger btn-sm"
                    aria-label="{t('Delete')} — {testCase.name}"
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
