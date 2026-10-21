<script lang="ts">
  import {
    CircleMinus,
    CirclePlus,
    Copy,
    Download,
    LayoutTemplate,
    MoveDown,
    MoveUp,
    Pencil,
    Plus,
    Trash2,
  } from '../lib/icons';
  import { api, type RuleBundle } from '../api/client';
  import { describeCondition, formatDecimal, swapped } from '../api/format';
  import {
    canonicalKey,
    facetOf,
    localFacets,
    nextPriority,
    withoutRepeats,
  } from '../api/conditions';
  import type {
    Category,
    Condition,
    ConditionCatalog,
    Instance,
    TemplateAdded,
    TemplateSummary,
    Rule,
    RuleDraft,
    RuleMediaType,
  } from '../api/types';
  import { createAsync } from '../lib/async.svelte';
  import { createOutcome } from '../lib/outcome.svelte';
  import { handFocus } from '../lib/focus';
  import { href } from '../lib/router.svelte';
  import { takeQueryFlag } from '../api/onboarding';
  import { i18n, t } from '../lib/i18n.svelte';
  import ActionMenu from '../components/ActionMenu.svelte';
  import EmptyState from '../components/EmptyState.svelte';
  import FileButton from '../components/FileButton.svelte';
  import ErrorBanner from '../components/ErrorBanner.svelte';
  import WarningBanner from '../components/WarningBanner.svelte';
  import RuleEditor from '../components/RuleEditor.svelte';
  import TemplateGallery from '../components/TemplateGallery.svelte';
  import TemplateDialog from '../components/TemplateDialog.svelte';
  import OutcomeBanner from '../components/OutcomeBanner.svelte';
  import GuideStepBanner from '../components/GuideStepBanner.svelte';
  import { ask, askConfirmation } from '../lib/confirm.svelte';
  import LibraryFacetsPanel from '../components/LibraryFacets.svelte';
  import TableSkeleton from '../components/TableSkeleton.svelte';
  import TableRegion from '../components/TableRegion.svelte';
  import { downloadJson } from '../lib/download';
  import { readJsonFile } from '../lib/upload';
  import { invalidateStatus } from '../lib/status.svelte';
  import { access } from '../lib/access.svelte';

  // Keyed on the union, so a media type added without a label fails the type check.
  const MEDIA_TYPE_KEY: Record<RuleMediaType, string> = {
    movie: 'MoviesOnly',
    series: 'SeriesOnly',
    both: 'Both',
  };

  const emptyDraft = (category: string, priority: number): RuleDraft => ({
    name: '',
    description: '',
    priority,
    enabled: true,
    media_type: 'both',
    conditions: [],
    exclusions: [],
    match_mode: 'all',
    target_category: category,
    instance_ids: null,
  });

  // Without a word for a failed diagnostic, the list reads as a healthy rule set.
  let analysisFailed = $state(false);

  const bundle = createAsync(async (signal) => {
    analysisFailed = false;
    const [rules, categories, instances, catalog, health] = await Promise.all([
      api.getRules(signal),
      api.getCategories(signal),
      api.getInstances(signal),
      api.getConditionCatalog(signal),
      // Tolerated: when the analysis fails, a list without badges beats no list at all.
      api.getRuleHealth(signal).catch(() => {
        analysisFailed = true;
        return null;
      }),
    ]);
    return { rules, categories, instances, catalog, health };
  });

  // Read once, since no action on a rule changes the library. Tolerated like the health report.
  const library = createAsync((signal) =>
    api.getLibraryFacets(signal).catch(() => {
      analysisFailed = true;
      return null;
    }),
  );

  const templates = createAsync((signal) => api.getTemplates(signal));
  let choosing = $state<TemplateSummary | null>(null);

  // Sent to `#templates` by the guide or the header, the gallery takes the focus once drawn.
  let templatesRequested = $state(window.location.hash === '#templates');
  $effect(() => {
    const requested = () => (templatesRequested = window.location.hash === '#templates');
    window.addEventListener('hashchange', requested);
    return () => window.removeEventListener('hashchange', requested);
  });
  $effect(() => {
    if (templatesRequested && templates.data && bundle.data) {
      templatesRequested = false;
      void handFocus('templates-title');
    }
  });

  async function templateAdded(template: TemplateSummary, added: TemplateAdded) {
    choosing = null;
    const message = t('TemplateAddedOutcome', {
      template: template.name,
      count: added.rules.length,
    });
    // A category made here has no folder yet, and nothing moves until it does.
    const unmapped = added.created.map((category) => t('TemplateCreatedUnmapped', { category }));
    if (unmapped.length > 0) outcome.warn(message, unmapped);
    else outcome.succeed(message);
    await Promise.all([reloadAfterWrite(), templates.reload()]);
  }

  const health = $derived(
    new Map((bundle.data?.health?.rules ?? []).map((entry) => [entry.rule_id, entry])),
  );

  let editing = $state<{ draft: RuleDraft; id?: string } | null>(null);
  const outcome = createOutcome();

  const rules = $derived<Rule[]>(bundle.data?.rules ?? []);
  const categories = $derived<Category[]>(bundle.data?.categories ?? []);
  const instances = $derived<Instance[]>(bundle.data?.instances ?? []);
  const instanceName = $derived(new Map(instances.map((instance) => [instance.id, instance.name])));
  const catalog = $derived<ConditionCatalog | undefined>(bundle.data?.catalog);
  const specByType = $derived(
    new Map((catalog?.conditions ?? []).map((spec) => [spec.type, spec])),
  );

  // The table names conditions and values as the editor does: `ja` reads as Japanese.
  const facets = $derived(library.data ? localFacets(library.data, i18n.language) : null);
  function nameIn(axis: string | undefined, value: string): string {
    const key = canonicalKey(value);
    return facetOf(facets, axis).find((facet) => canonicalKey(facet.value) === key)?.label ?? value;
  }

  function describe(condition: Condition): string {
    const spec = specByType.get(condition.type);
    return describeCondition(condition, {
      label: spec?.label,
      phrase: (key, params) => t(key, params),
      separator: t('ListSeparator'),
      empty: t('NoneSpoken'),
      name: (value) => nameIn(spec?.suggestions, value),
      scale: (id) => catalog?.score_scales.find((scale) => scale.id === id)?.name ?? id,
      number: (value) => formatDecimal(value, i18n.language),
    });
  }

  function openCreate() {
    const priorities = rules.map((rule) => rule.priority);
    editing = { draft: emptyDraft(categories[0]?.name ?? 'standard', nextPriority(priorities)) };
  }

  // `?new=1` opens the editor once the bundle it is built from is in, as the button does.
  let createRequested = takeQueryFlag('new');
  $effect(() => {
    if (createRequested && bundle.data) {
      createRequested = false;
      if (!access.readOnly) openCreate();
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
        conditions: rule.conditions.map(withoutRepeats),
        exclusions: rule.exclusions.map(withoutRepeats),
        match_mode: rule.match_mode,
        target_category: rule.target_category,
        instance_ids: rule.instance_ids,
      },
    };
  }

  // Without the revision, the guide's rule step stays open until the next navigation.
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

  // After a delete the focus goes to the rule in its place, else the one before, else the table.
  const menuId = (index: number) => `rules-menu-${index}`;

  async function move(index: number, direction: -1 | 1) {
    const from = rules[index];
    const reordered = swapped(rules, index, direction);
    if (!from || !reordered) return;
    await act(() => api.reorderRules(reordered.map((rule) => rule.id)), t('PrioritiesUpdated'));
    // The focus stays on the pressed arrow, or the other once the pressed one turns disabled.
    const [pressed, other] = direction === -1 ? ['raise', 'lower'] : ['lower', 'raise'];
    void handFocus(`rule-${from.id}-${pressed}`, `rule-${from.id}-${other}`);
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
      const parsed = (await readJsonFile(file)) as RuleBundle;
      // Three answers, so not `confirm()`: Cancel and Escape would each map to an import.
      const answer = await ask(t('ImportReplaceQuestion'), [
        { label: 'ImportAppend', value: 'append' },
        { label: 'ImportReplace', value: 'replace', danger: true },
      ]);
      if (answer === null) return;
      const result = await api.importRules(parsed, answer === 'replace');
      const summary = result.skipped.length
        ? t('ImportResultWithSkipped', {
            count: result.imported,
            skipped: result.skipped.length,
          })
        : t('ImportResult', { count: result.imported });
      // A rule imported switched off or on fewer instances still needs someone to look at it.
      const details = [...result.skipped, ...result.adjusted];
      if (details.length === 0) outcome.succeed(summary);
      else if (result.imported === 0) outcome.fail(summary, details);
      else outcome.warn(summary, details);
      await reloadAfterWrite();
    } catch (err) {
      outcome.fail(err);
    }
  }
</script>

{#snippet copyIcon()}<Copy size={14} aria-hidden="true" />{/snippet}
{#snippet trashIcon()}<Trash2 size={14} aria-hidden="true" />{/snippet}

<div>
  <div class="page-header">
    <div>
      <h1 class="page-title">{t('Rules')}</h1>
      <p class="page-subtitle">{t('RulesSubtitle')}</p>
    </div>
    <div class="flex gap-2">
      {#if !access.readOnly}
        <FileButton
          label={t('Import')}
          accept="application/json"
          onFile={(file) => void importBundle(file)}
        />
      {/if}
      <button class="btn btn-secondary" onclick={exportBundle} disabled={rules.length === 0}>
        <Download size={16} />
        {t('Export')}
      </button>
      {#if !access.readOnly}
        {#if templates.data && rules.length > 0}
          <a class="btn btn-secondary" href={href('/rules#templates')}>
            <LayoutTemplate size={16} />
            {t('Templates')}
          </a>
        {/if}
        <button id="rules-new" class="btn btn-primary" onclick={openCreate}>
          <Plus size={16} />
          {t('NewRule')}
        </button>
      {/if}
    </div>
  </div>

  <ErrorBanner
    message={bundle.error}
    onDismiss={() => (bundle.error = null)}
    onRetry={() => void bundle.reload()}
  />
  <OutcomeBanner {outcome} />
  {#if analysisFailed}
    <WarningBanner
      message={t('RuleAnalysisUnavailable')}
      onRetry={() => void Promise.all([bundle.reload(), library.reload()])}
    />
  {/if}
  <GuideStepBanner step="rule" />

  <!-- Above the rules: a rule written against a value the library does not carry
       matches nothing, and looks like one that should. -->
  {#if library.data}
    <LibraryFacetsPanel facets={library.data} captionOf={(type) => specByType.get(type)?.label} />
  {/if}

  {#if templates.data && rules.length === 0 && !bundle.loading && !access.readOnly}
    <TemplateGallery templates={templates.data} onChoose={(template) => (choosing = template)} />
  {/if}
  <ErrorBanner message={templates.error} onRetry={() => void templates.reload()} />

  <div class="card">
    <TableRegion label={t('Rules')} id="rules-table">
      <table>
        <caption class="visually-hidden">{t('Rules')}</caption>
        <thead>
          <tr>
            <th class="w-90">{t('Priority')}</th>
            <th>{t('Rule')}</th>
            <th>{t('Target')}</th>
            <th>{t('Scope')}</th>
            <th>{t('Logic')}</th>
            <th>{t('Conditions')}</th>
            <th class="w-190"><span class="visually-hidden">{t('Actions')}</span></th>
          </tr>
        </thead>
        <tbody>
          <!-- First load only: on a reload the skeleton would take the rows and the focus. -->
          {#if bundle.loading && rules.length === 0}
            <TableSkeleton columns={7} />
          {:else if rules.length === 0 && !bundle.error}
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
                      {t('RuleDisabled')}
                    </span>
                  {/if}
                  {#if verdict?.shadowed_by}
                    <!-- The rule taking its items is the one to move, so it is named. -->
                    <span class="badge badge-warning ms-2">
                      {t('RuleShadowed', { rule: verdict.shadowed_by })}
                    </span>
                  {:else if verdict?.duplicate_of}
                    <!-- Same conditions and target: one of the two decides nothing. -->
                    <span class="badge badge-warning ms-2">
                      {t('RuleDuplicateOf', { rule: verdict.duplicate_of })}
                    </span>
                  {:else if verdict?.matched_nothing && rule.enabled}
                    <!-- Muted: the fault is too narrow a condition, which no priority fixes. -->
                    <span class="badge badge-value muted ms-2">
                      {t('RuleMatchedNothing')}
                    </span>
                  {/if}
                  {#if verdict?.ambiguous_with}
                    <!-- A tie broken by id is its own fault, shown beside any other. -->
                    <span class="badge badge-warning ms-2">
                      {t('RuleAmbiguous')}
                    </span>
                  {/if}
                  {#if rule.description}
                    <div class="text-muted text-sm">{rule.description}</div>
                  {/if}
                  {#if verdict?.shadowed_by}
                    <div class="text-muted text-sm">
                      {t('RuleShadowedHint', {
                        count: verdict.shadowed,
                        rule: verdict.shadowed_by,
                      })}
                    </div>
                  {/if}
                </td>
                <td><span class="badge badge-value">{rule.target_category}</span></td>
                <td>
                  <div class="flex flex-wrap gap-1">
                    <span class="badge badge-value muted">{t(MEDIA_TYPE_KEY[rule.media_type])}</span
                    >
                    {#each rule.instance_ids ?? [] as id (id)}
                      <span class="badge badge-value">{instanceName.get(id) ?? id}</span>
                    {/each}
                  </div>
                </td>
                <td>
                  <span class="badge badge-value muted">
                    {t(rule.match_mode === 'any' ? 'LogicAny' : 'LogicAll')}
                  </span>
                </td>
                <td>
                  <!-- A plus to match, a minus to veto, one size so they pair. The word beside
                       the minus keeps its colour from being the only sign of "except". -->
                  <ul class="rule-conditions">
                    {#each rule.conditions as condition, i (i)}
                      <li class="rule-condition">
                        <CirclePlus size={14} class="rule-condition-mark" aria-hidden="true" />
                        <span>{describe(condition)}</span>
                      </li>
                    {/each}
                    {#each rule.exclusions as condition, i (i)}
                      <li class="rule-condition is-excluded">
                        <CircleMinus size={14} class="rule-condition-mark" aria-hidden="true" />
                        <span class="rule-condition-except">{t('ExceptPrefix')}</span>
                        <span>{describe(condition)}</span>
                      </li>
                    {/each}
                  </ul>
                </td>
                <td>
                  {#if !access.readOnly}
                    <div class="flex gap-2">
                      <button
                        id="rule-{rule.id}-raise"
                        class="btn btn-secondary btn-sm"
                        disabled={index === 0}
                        title={t('RaisePriority')}
                        aria-label="{t('RaisePriority')} – {rule.name}"
                        onclick={() => void move(index, -1)}
                      >
                        <MoveUp size={14} />
                      </button>
                      <button
                        id="rule-{rule.id}-lower"
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
                      <ActionMenu
                        id={menuId(index)}
                        label="{t('Actions')} – {rule.name}"
                        actions={[
                          {
                            label: t('Duplicate'),
                            icon: copyIcon,
                            onSelect: () =>
                              void act(() => api.duplicateRule(rule.id), t('RuleDuplicated')),
                          },
                          {
                            label: t('Delete'),
                            icon: trashIcon,
                            danger: true,
                            onSelect: async () => {
                              if (
                                await askConfirmation(
                                  t('ConfirmDeleteRule', { name: rule.name }),
                                  'Delete',
                                )
                              ) {
                                await act(() => api.deleteRule(rule.id), t('RuleDeleted'));
                                void handFocus(menuId(index), menuId(index - 1), 'rules-table');
                              }
                            },
                          },
                        ]}
                      />
                    </div>
                  {/if}
                </td>
              </tr>
            {/each}
          {/if}
        </tbody>
      </table>
    </TableRegion>
  </div>

  {#if templates.data && rules.length > 0 && !access.readOnly}
    <TemplateGallery templates={templates.data} onChoose={(template) => (choosing = template)} />
  {/if}

  {#if choosing}
    {@const template = choosing}
    <TemplateDialog
      {template}
      {categories}
      languageOptions={facetOf(facets, 'original_languages')}
      profileOptions={facetOf(facets, 'quality_profiles')}
      onClose={() => (choosing = null)}
      onAdded={(added) => void templateAdded(template, added)}
    />
  {/if}

  {#if editing && catalog}
    <RuleEditor
      knownFacets={library.data ?? null}
      draft={editing.draft}
      ruleId={editing.id}
      {categories}
      {instances}
      {catalog}
      onClose={() => (editing = null)}
      returnFocus="rules-new"
      onSaved={async (message) => {
        editing = null;
        outcome.succeed(message);
        await reloadAfterWrite();
      }}
    />
  {/if}
</div>
