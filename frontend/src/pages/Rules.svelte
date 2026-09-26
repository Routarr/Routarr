<script lang="ts">
  import {
    CircleMinus,
    CirclePlus,
    Copy,
    Download,
    MoveDown,
    MoveUp,
    Pencil,
    Plus,
    Trash2,
    Upload,
  } from '../lib/icons';
  import { api, type RuleBundle } from '../api/client';
  import { describeCondition } from '../api/format';
  import { canonicalKey, localFacets } from '../api/conditions';
  import type {
    Category,
    Condition,
    ConditionCatalog,
    Facet,
    FacetAxis,
    Rule,
    RuleDraft,
    RuleMediaType,
    Vocabularies,
  } from '../api/types';
  import { createAsync } from '../lib/async.svelte';
  import { createOutcome } from '../lib/outcome.svelte';
  import { takeQueryFlag } from '../api/onboarding';
  import { i18n, t } from '../lib/i18n.svelte';
  import EmptyState from '../components/EmptyState.svelte';
  import ErrorBanner from '../components/ErrorBanner.svelte';
  import Loading from '../components/Loading.svelte';
  import RuleEditor from '../components/RuleEditor.svelte';
  import OutcomeBanner from '../components/OutcomeBanner.svelte';
  import GuideStepBanner from '../components/GuideStepBanner.svelte';
  import { ask, askConfirmation } from '../lib/confirm.svelte';
  import LibraryFacetsPanel from '../components/LibraryFacets.svelte';
  import TableRegion from '../components/TableRegion.svelte';
  import { downloadJson } from '../lib/download';
  import { invalidateStatus } from '../lib/status.svelte';

  // Keyed on the union rather than on `string`: a media type added to
  // `RuleMediaType` without a label here then fails the type check instead
  // of rendering an empty badge.
  const MEDIA_TYPE_KEY: Record<RuleMediaType, string> = {
    movie: 'MoviesOnly',
    series: 'SeriesOnly',
    both: 'Both',
  };

  const emptyDraft = (category: string): RuleDraft => ({
    name: '',
    description: '',
    priority: 100,
    enabled: true,
    media_type: 'both',
    conditions: [],
    exclusions: [],
    match_mode: 'all',
    target_category: category,
    instance_ids: null,
  });

  const bundle = createAsync(async (signal) => {
    const [rules, categories, catalog, health, facets] = await Promise.all([
      api.getRules(signal),
      api.getCategories(signal),
      api.getConditionCatalog(signal),
      // Which rules are actually deciding anything. Validation looks inside one
      // rule and nothing else looks between them, which is where
      // first-match-by-priority puts its one trap — a rule under a broader one
      // can never fire, and
      // the preview reports that as "0 changes", the same as a rule that
      // correctly changes nothing.
      //
      // Tolerated rather than awaited hard: the report evaluates the whole
      // library, and a rule list that refuses to render because a diagnostic
      // failed is worse than a rule list without badges.
      api.getRuleHealth(signal).catch(() => null),
      // Same tolerance, same reason: an aggregation that failed must not take
      // the rule list down with it.
      api.getLibraryFacets(signal).catch(() => null),
    ]);
    return { rules, categories, catalog, health, facets };
  });

  const health = $derived(
    new Map((bundle.data?.health?.rules ?? []).map((entry) => [entry.rule_id, entry])),
  );

  let editing = $state<{ draft: RuleDraft; id?: string } | null>(null);
  const outcome = createOutcome();

  const rules = $derived<Rule[]>(bundle.data?.rules ?? []);
  const categories = $derived<Category[]>(bundle.data?.categories ?? []);
  const catalog = $derived<ConditionCatalog | undefined>(bundle.data?.catalog);
  const specByType = $derived(
    new Map((catalog?.conditions ?? []).map((spec) => [spec.type, spec])),
  );

  // The table reads as the editor does: a condition by its caption, a value by
  // the name the library or its vocabulary gives it, `ja` as Japanese.
  const facets = $derived(
    bundle.data?.facets ? localFacets(bundle.data.facets, i18n.language) : null,
  );
  function nameIn(axis: string | undefined, value: string): string {
    if (!axis || !facets) return value;
    const key = canonicalKey(value);
    const known: Facet[] = [
      ...(facets[axis as FacetAxis] ?? []),
      ...(facets.vocabularies[axis as keyof Vocabularies] ?? []),
    ];
    return known.find((facet) => canonicalKey(facet.value) === key)?.label ?? value;
  }

  function describe(condition: Condition): string {
    const spec = specByType.get(condition.type);
    return describeCondition(condition, {
      label: spec?.label,
      separator: t('ListSeparator'),
      empty: t('None'),
      name: (value) => nameIn(spec?.suggestions, value),
    });
  }

  function openCreate() {
    editing = { draft: emptyDraft(categories[0]?.name ?? 'standard') };
  }

  // The guide's "Create a rule" lands here. The editor waits for the bundle, as
  // the button does: it is built from the catalogue and the categories.
  let createRequested = takeQueryFlag('new');
  $effect(() => {
    if (createRequested && bundle.data) {
      createRequested = false;
      openCreate();
    }
  });

  function openEdit(rule: Rule) {
    editing = {
      id: rule.id,
      draft: {
        name: rule.name,
        description: rule.description ?? '',
        priority: rule.priority,
        enabled: rule.enabled,
        media_type: rule.media_type,
        conditions: rule.conditions,
        exclusions: rule.exclusions ?? [],
        match_mode: rule.match_mode ?? 'all',
        target_category: rule.target_category,
        instance_ids: rule.instance_ids,
      },
    };
  }

  // The guide the shell draws reads its rule step from the rules. Without the
  // revision, the step shows open until the next navigation.
  async function reloadAfterWrite() {
    invalidateStatus();
    await bundle.reload();
  }

  async function act(fn: () => Promise<unknown>, message: string) {
    try {
      await fn();
      outcome.succeed(message);
      await reloadAfterWrite();
    } catch (err) {
      outcome.fail(err);
    }
  }

  async function move(index: number, direction: -1 | 1) {
    const target = index + direction;
    if (target < 0 || target >= rules.length) return;
    const reordered = [...rules];
    const from = reordered[index];
    const to = reordered[target];
    // The bounds check above is not what makes this safe — a sparse array would
    // pass it and still hand back `undefined`. Reading both first is.
    if (!from || !to) return;
    reordered[index] = to;
    reordered[target] = from;
    await act(() => api.reorderRules(reordered.map((rule) => rule.id)), t('PrioritiesUpdated'));
  }

  async function exportBundle() {
    try {
      downloadJson(await api.exportRules(), 'routarr-rules.json');
      outcome.clear();
    } catch (err) {
      outcome.fail(err);
    }
  }

  async function importBundle(file: File) {
    try {
      const parsed = JSON.parse(await file.text()) as RuleBundle;
      // Three outcomes, which is why a `confirm()` cannot ask this: mapping one
      // of them onto Cancel makes Cancel import the file and Escape import it
      // silently, with no way to abort at all.
      const answer = await ask(t('ImportReplaceQuestion'), [
        { label: 'ImportAppend', value: 'append' },
        { label: 'ImportReplace', value: 'replace', danger: true },
      ]);
      if (answer === null) return;
      const result = await api.importRules(parsed, answer === 'replace');
      const summary =
        t('ImportResult', { count: result.imported }) +
        (result.skipped.length ? t('ImportSkipped', { count: result.skipped.length }) : '');
      if (result.skipped.length === 0) outcome.succeed(summary);
      else if (result.imported === 0) outcome.fail(summary, result.skipped);
      else outcome.warn(summary, result.skipped);
      await reloadAfterWrite();
    } catch (err) {
      outcome.fail(err instanceof SyntaxError ? t('NotValidJson') : err);
    }
  }
</script>

<div>
  <div class="page-header">
    <div>
      <h1 class="page-title">{t('RulesEngine')}</h1>
      <p class="page-subtitle">{t('RulesSubtitle')}</p>
    </div>
    <div class="flex gap-2">
      <label class="btn btn-secondary cursor-pointer">
        <Upload size={16} />
        {t('Import')}
        <input
          type="file"
          accept="application/json"
          hidden
          onchange={(event) => {
            const file = event.currentTarget.files?.[0];
            if (file) void importBundle(file);
            event.currentTarget.value = '';
          }}
        />
      </label>
      <button class="btn btn-secondary" onclick={exportBundle} disabled={rules.length === 0}>
        <Download size={16} />
        {t('Export')}
      </button>
      <button class="btn btn-primary" onclick={openCreate}>
        <Plus size={16} />
        {t('NewRule')}
      </button>
    </div>
  </div>

  <ErrorBanner
    message={bundle.error}
    onDismiss={() => (bundle.error = null)}
    onRetry={() => void bundle.reload()}
  />
  <OutcomeBanner {outcome} />
  <GuideStepBanner step="rule" />

  <!-- Before the rules, not after: it is what you consult in order to write
       one, and a rule written against a value the library does not carry
       matches nothing while looking exactly like a rule that should. -->
  {#if bundle.data?.facets}
    <LibraryFacetsPanel
      facets={bundle.data.facets}
      captionOf={(type) => specByType.get(type)?.label}
    />
  {/if}

  <div class="card">
    <TableRegion label={t('RulesEngine')}>
      <table>
        <caption class="visually-hidden">{t('RulesEngine')}</caption>
        <thead>
          <tr>
            <th class="w-90">{t('Priority')}</th>
            <th>{t('Rule')}</th>
            <th>{t('Target')}</th>
            <th>{t('Scope')}</th>
            <th>{t('Logic')}</th>
            <th>{t('Conditions')}</th>
            <th class="w-190">{t('Actions')}</th>
          </tr>
        </thead>
        <tbody>
          {#if bundle.loading}
            <tr><td colspan="7"><Loading /></td></tr>
          {:else if rules.length === 0}
            <tr><td colspan="7"><EmptyState>{t('NoRulesYet')}</EmptyState></td></tr>
          {:else}
            {#each rules as rule, index (rule.id)}
              {@const verdict = health.get(rule.id)}
              <tr class:row-muted={!rule.enabled}>
                <td><span class="num">#{rule.priority}</span></td>
                <td>
                  <strong>{rule.name}</strong>
                  {#if !rule.enabled}
                    <span class="badge badge-warning ms-2">
                      {t('Disabled')}
                    </span>
                  {/if}
                  {#if verdict?.shadowed_by}
                    <!-- Naming the culprit is the whole point: a rule that
                         decides nothing is a fact, but the rule taking its
                         items is the thing you can move. -->
                    <span
                      class="badge badge-warning ms-2"
                      title={t('RuleShadowedHint', {
                        count: verdict.shadowed,
                        rule: verdict.shadowed_by,
                      })}
                    >
                      {t('RuleShadowed', { rule: verdict.shadowed_by })}
                    </span>
                  {:else if verdict?.duplicate_of}
                    <!-- Identical conditions and target: one of the two decides
                         nothing whatever the priorities say. -->
                    <span class="badge badge-warning ms-2">
                      {t('RuleDuplicateOf', { rule: verdict.duplicate_of })}
                    </span>
                  {:else if verdict?.matched_nothing && rule.enabled}
                    <!-- A different fault, wanting a different fix: too narrow
                         a condition rather than too low a priority. -->
                    <span class="badge badge-value muted ms-2">
                      {t('RuleMatchedNothing')}
                    </span>
                  {/if}
                  {#if verdict?.ambiguous_with}
                    <!-- Shown alongside whatever else is true of the rule: the
                         tie-break falling to an id is a separate fault from the
                         rule being shadowed or dead. -->
                    <span class="badge badge-warning ms-2">
                      {t('RuleAmbiguous')}
                    </span>
                  {/if}
                  {#if rule.description}
                    <div class="text-muted text-sm">{rule.description}</div>
                  {/if}
                </td>
                <td><span class="badge badge-value">{rule.target_category}</span></td>
                <td
                  ><span class="badge badge-value muted">{t(MEDIA_TYPE_KEY[rule.media_type])}</span
                  ></td
                >
                <td>
                  <span class="badge badge-value muted">
                    {t(rule.match_mode === 'any' ? 'LogicAny' : 'LogicAll')}
                  </span>
                </td>
                <td>
                  <!-- A plus to match and a minus to veto: one shape at one size,
                       so the two read as a pair, and a word beside the minus so
                       its colour is never the only thing that says "except". -->
                  <ul class="rule-conditions">
                    {#each rule.conditions as condition, i (i)}
                      <li class="rule-condition">
                        <CirclePlus size={14} class="rule-condition-mark" aria-hidden="true" />
                        <span>{describe(condition)}</span>
                      </li>
                    {/each}
                    {#each rule.exclusions ?? [] as condition, i (i)}
                      <li class="rule-condition is-excluded">
                        <CircleMinus size={14} class="rule-condition-mark" aria-hidden="true" />
                        <span class="rule-condition-except">{t('ExceptPrefix')}</span>
                        <span>{describe(condition)}</span>
                      </li>
                    {/each}
                  </ul>
                </td>
                <td>
                  <div class="flex gap-2">
                    <button
                      class="btn btn-secondary btn-sm"
                      disabled={index === 0}
                      title={t('RaisePriority')}
                      aria-label="{t('RaisePriority')} – {rule.name}"
                      onclick={() => void move(index, -1)}
                    >
                      <MoveUp size={14} />
                    </button>
                    <button
                      class="btn btn-secondary btn-sm"
                      disabled={index === rules.length - 1}
                      title={t('LowerPriority')}
                      aria-label="{t('LowerPriority')} – {rule.name}"
                      onclick={() => void move(index, 1)}
                    >
                      <MoveDown size={14} />
                    </button>
                    <button
                      class="btn btn-secondary btn-sm"
                      title={t('Edit')}
                      aria-label="{t('Edit')} – {rule.name}"
                      onclick={() => openEdit(rule)}
                    >
                      <Pencil size={14} />
                    </button>
                    <button
                      class="btn btn-secondary btn-sm"
                      title={t('Duplicate')}
                      aria-label="{t('Duplicate')} – {rule.name}"
                      onclick={() =>
                        void act(() => api.duplicateRule(rule.id), t('RuleDuplicated'))}
                    >
                      <Copy size={14} />
                    </button>
                    <button
                      class="btn btn-danger btn-sm"
                      title={t('Delete')}
                      aria-label="{t('Delete')} – {rule.name}"
                      onclick={async () => {
                        if (
                          await askConfirmation(
                            t('ConfirmDeleteRule', { name: rule.name }),
                            'Delete',
                          )
                        ) {
                          void act(() => api.deleteRule(rule.id), t('RuleDeleted'));
                        }
                      }}
                    >
                      <Trash2 size={14} />
                    </button>
                  </div>
                </td>
              </tr>
            {/each}
          {/if}
        </tbody>
      </table>
    </TableRegion>
  </div>

  {#if editing && catalog}
    <RuleEditor
      knownFacets={bundle.data?.facets ?? null}
      draft={editing.draft}
      ruleId={editing.id}
      {categories}
      {catalog}
      onClose={() => (editing = null)}
      onSaved={async (message) => {
        editing = null;
        outcome.succeed(message);
        await reloadAfterWrite();
      }}
    />
  {/if}
</div>
