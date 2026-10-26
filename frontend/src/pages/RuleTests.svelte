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
  import { CheckCircle2, Play, Trash2 } from '../lib/icons';
  import type { RuleTest, RuleTestResult, RuleTestRun } from '../api/types';
  import { takeQueryFlag } from '../api/onboarding';
  import { createExplainer } from '../lib/explain.svelte';
  import NoValue from '../components/NoValue.svelte';
  import EmptyState from '../components/EmptyState.svelte';
  import ErrorBanner from '../components/ErrorBanner.svelte';
  import OutcomeBanner from '../components/OutcomeBanner.svelte';
  import TableSkeleton from '../components/TableSkeleton.svelte';
  import TableRegion from '../components/TableRegion.svelte';
  import Pager from '../components/Pager.svelte';
  import ActionMenu from '../components/ActionMenu.svelte';
  import ExplanationModal from '../components/ExplanationModal.svelte';
  import WhyButton from '../components/WhyButton.svelte';

  /**
   * Pinned expectations, replayed against the rules as they stand: the first match by priority
   * wins, so one new rule can quietly move what every rule below it routed.
   */
  let page = $state(1);
  // The failing-tests warning leads here with `?failing=1`.
  let failing = $state(takeQueryFlag('failing'));
  const cases = createAsync(
    (signal) =>
      api.getRuleTests({ page, per_page: 50, ...(failing ? { failing: true } : {}) }, signal),
    () => [page, failing],
  );
  const explainer = createExplainer();
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

  const menuId = (index: number) => `rule-tests-menu-${index}`;

  // A deletion takes its row with it, so the focus goes to the nearest row's menu left.
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
      void handFocus(menuId(index), menuId(index - 1), 'rule-tests-table');
    } catch (err) {
      outcome.fail(err);
    }
  }

  /**
   * Where a failing test's title goes now, from this visit's run, else from the last one. A
   * test that fails because its copy cannot be read has none.
   */
  function nowGoesTo(testCase: RuleTest): string | null {
    const verdict = verdictOf(testCase.id);
    if (verdict) return verdict.passed || verdict.error ? null : verdict.actual_category;
    return testCase.last_passed === false ? testCase.last_actual_category : null;
  }

  /** Accepts the routing a deliberate rule change gave the title, asked first. */
  async function expectNow(testCase: RuleTest, category: string, index: number) {
    const question = t('ConfirmExpectCategory', {
      name: testCase.name,
      category,
      expected: testCase.expected_category,
    });
    if (!(await askConfirmation(question, 'ChangeExpectation', false))) return;
    try {
      await api.setRuleTestExpectation(testCase.id, category);
      run = null;
      invalidateStatus();
      outcome.succeed(t('RuleTestExpectationChanged', { name: testCase.name, category }));
      await cases.reload();
      // Settled, the test offers the change no more: the focus goes to its menu.
      void handFocus(menuId(index), 'rule-tests-table');
    } catch (err) {
      outcome.fail(err);
    }
  }

  function verdictOf(id: string): RuleTestResult | undefined {
    return verdicts.get(id);
  }
</script>

{#snippet trashIcon()}<Trash2 size={14} aria-hidden="true" />{/snippet}

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
  <ErrorBanner message={explainer.failure} onDismiss={explainer.dismiss} />

  {#if failing || total > 0}
    <div class="toolbar">
      <label class="flex items-center gap-2 text-md">
        <input
          type="checkbox"
          checked={failing}
          onchange={(event) => {
            page = 1;
            failing = event.currentTarget.checked;
          }}
        />
        {t('FailingOnly')}
      </label>
    </div>
  {/if}

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
                <EmptyState>{t(failing ? 'NoRuleTestFailing' : 'NoRuleTests')}</EmptyState>
              </td>
            </tr>
          {:else}
            {#each listed as testCase, index (testCase.id)}
              {@const verdict = verdictOf(testCase.id)}
              {@const elsewhere = nowGoesTo(testCase)}
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
                    {#if elsewhere}
                      <span class="text-muted ms-2">
                        {t('RuleTestNowGoesTo', { category: elsewhere })}
                      </span>
                    {/if}
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
                  <div class="row-actions">
                    {#if testCase.source_media_id}
                      <WhyButton
                        {explainer}
                        mediaId={testCase.source_media_id}
                        title={testCase.name}
                      />
                    {/if}
                    {#if !access.readOnly}
                      {#if elsewhere && elsewhere !== testCase.expected_category}
                        <button
                          class="btn btn-secondary btn-sm"
                          aria-label="{t('ExpectCategory', {
                            category: elsewhere,
                          })} – {testCase.name}"
                          onclick={() => void expectNow(testCase, elsewhere, index)}
                        >
                          <CheckCircle2 size={14} aria-hidden="true" />
                          {t('ExpectCategory', { category: elsewhere })}
                        </button>
                      {/if}
                      <ActionMenu
                        id={menuId(index)}
                        label="{t('Actions')} – {testCase.name}"
                        actions={[
                          {
                            label: t('Delete'),
                            icon: trashIcon,
                            danger: true,
                            onSelect: () => void remove(testCase.id, testCase.name, index),
                          },
                        ]}
                      />
                    {/if}
                  </div>
                </td>
              </tr>
            {/each}
          {/if}
        </tbody>
      </table>
    </TableRegion>
    <Pager pagination={cases.data?.pagination} bind:page countKey="RuleTestCount" />
  </div>

  <!-- Read again once closed: the panel may have pinned another test. -->
  {#if explainer.shown}
    <ExplanationModal
      data={explainer.shown}
      onClose={() => {
        explainer.close();
        void cases.reload();
      }}
    />
  {/if}
</div>
