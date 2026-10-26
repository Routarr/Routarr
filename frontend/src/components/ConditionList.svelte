<script lang="ts">
  import { tick, type Snippet } from 'svelte';
  import { Plus, Trash2 } from '../lib/icons';
  import type {
    Condition,
    ConditionSpec,
    LibraryFacets,
    ScoreScale,
    ValidationIssue,
  } from '../api/types';
  import { i18n, t } from '../lib/i18n.svelte';
  import { facetOf, localFacets } from '../api/conditions';
  import { phraseKey } from '../api/format';
  import { handFocus } from '../lib/focus';
  import ActionMenu from './ActionMenu.svelte';
  import ConditionValue from './ConditionValue.svelte';
  import FieldIssues from './FieldIssues.svelte';

  let {
    title,
    list,
    conditions,
    keys,
    specs,
    addable,
    facets = null,
    facetsLoading = false,
    facetsError = null,
    scales = [],
    errorsOf = () => [],
    control,
    onAdd,
    onRetype,
    onUpdate,
    onRemove,
  }: {
    title: string;
    list: 'conditions' | 'exclusions';
    conditions: Condition[];
    /** One per condition, which keeps a row's own state with it when another is removed. */
    keys: number[];
    /**
     * Every spec, unfiltered: a rule saved before its media type narrowed still displays,
     * and a condition with no spec shows only its raw type.
     */
    specs: ConditionSpec[];
    /** The specs the add menu offers for this rule's media type. */
    addable: ConditionSpec[];
    /** The library's values per axis, offered as suggestions. Null while they load. */
    facets?: LibraryFacets | null;
    facetsLoading?: boolean;
    facetsError?: string | null;
    /** The scales a score range names. */
    scales?: ScoreScale[];
    /** The server's errors about the control an id names: a row, `rules-{list}-{index}`. */
    errorsOf?: (field: string) => ValidationIssue[];
    /** What the caption carries beside it, as the conditions carry their logic. */
    control?: Snippet;
    onAdd: (type: string) => void;
    /** Swap a condition for its counterpart with the other quantifier, keeping its values. */
    onRetype: (index: number, type: string) => void;
    /** `unreadable` when the field holds text `value` could not take in. */
    onUpdate: (index: number, value: unknown, unreadable: boolean) => void;
    onRemove: (index: number) => void;
  } = $props();

  const named = $derived(facets ? localFacets(facets, i18n.language) : null);
  // A list with no condition is refused at its add button.
  const adding = $derived(errorsOf(`rules-${list}-add`));

  // A quantified pair is captioned by its `any` half, since the selector already says "all of".
  function caption(spec?: ConditionSpec): string | undefined {
    if (spec?.quantifier !== 'all') return spec?.label;
    return specs.find((candidate) => candidate.type === spec.counterpart)?.label ?? spec.label;
  }

  // One half of each pair is offered, and the row's selector reaches the other.
  const offerable = $derived(addable.filter((spec) => spec.quantifier !== 'all'));
  // A long list of conditions is read by family, in the order the catalogue gives them.
  const FAMILIES: Record<ConditionSpec['group'], string> = {
    metadata: 'ConditionGroupMetadata',
    arr: 'ConditionGroupArr',
    title: 'ConditionGroupTitle',
    folder: 'ConditionGroupFolder',
  };
  const families = $derived(
    (Object.keys(FAMILIES) as ConditionSpec['group'][])
      .map((group) => ({ group, specs: offerable.filter((spec) => spec.group === group) }))
      .filter((family) => family.specs.length > 0),
  );

  /**
   * A menu rather than a select: on Windows and Linux an arrow key on a closed select fires
   * `change`, which would add a condition per press.
   */
  const choices = $derived(
    families.flatMap((family) =>
      family.specs.map((spec) => ({
        // Flagged only when no enabled source, the Arr included, can answer it.
        label: spec.available
          ? spec.label
          : t('SideBySide', { first: spec.label, second: t('NeedsMetadataSuffix') }),
        group: t(FAMILIES[family.group]),
        onSelect: () => void add(spec.type),
      })),
    ),
  );

  async function add(type: string) {
    const index = conditions.length;
    onAdd(type);
    await tick();
    document
      .getElementById(`rules-${list}-${index}`)
      ?.querySelector<HTMLElement>('.condition-value :is(input, select, textarea)')
      ?.focus();
  }

  /** The focus goes to the row that moves up into the deleted one's place, else the add button. */
  function remove(index: number) {
    onRemove(index);
    void handFocus(`rules-${list}-${index}-delete`, `rules-${list}-add`);
  }

  /**
   * A list condition's whole sentence with a gap for its values, since each language puts
   * "any of", "all of" and "none of" where its own grammar wants them.
   */
  const sentence = (type: string) => t(phraseKey(type), { values: '…' });

  const listed = (spec?: ConditionSpec) =>
    spec?.value_type === 'string_list' || spec?.value_type === 'number_list';
</script>

<!-- A group heading rather than a `<label>`, which names exactly one control. -->
<div class="form-group" role="group" aria-labelledby="rules-{list}-caption">
  <div class="condition-list-head">
    <span class="form-label" id="rules-{list}-caption">{title}</span>
    {@render control?.()}
  </div>
  {#if list === 'conditions'}
    <p class="text-muted text-sm mt-1">{t('ConditionValuesHelp')}</p>
  {:else}
    <!-- Explained here so the caption stays short enough not to fold on a phone. -->
    <p class="text-muted text-sm mt-1">{t('ExclusionsHelp')}</p>
  {/if}

  <div class="flex flex-col gap-2 mt-2">
    {#each conditions as condition, index (keys[index] ?? index)}
      {@const spec = specs.find((candidate) => candidate.type === condition.type)}
      {@const field = `rules-${list}-${index}`}
      {@const refused = errorsOf(field)}
      <div id={field} class="condition-row {list === 'exclusions' ? 'excluded' : ''}">
        {#if spec?.counterpart}
          {@const any = spec.quantifier === 'all' ? spec.counterpart : spec.type}
          {@const all = spec.quantifier === 'all' ? spec.type : spec.counterpart}
          <select
            class="form-select condition-caption min-w-190"
            aria-label="{t('QuantifierLabel')} – {caption(spec)}"
            value={spec.quantifier}
            onchange={(event) =>
              onRetype(index, event.currentTarget.value === 'all' ? spec.counterpart : spec.type)}
          >
            <option value="any">{sentence(any)}</option>
            <option value="all">{sentence(all)}</option>
          </select>
        {:else if listed(spec)}
          <!-- With no counterpart the media holds one value, so several are alternatives.
               The sentence says so, and the field reads it out as its description. -->
          <span class="condition-caption min-w-190 text-md" id="rules-{list}-{index}-sentence"
            >{sentence(condition.type)}</span
          >
        {:else}
          <span class="condition-caption min-w-190 text-md">{caption(spec) ?? condition.type}</span>
        {/if}
        <div class="condition-value flex-1">
          <ConditionValue
            {spec}
            describedBy={[
              !spec?.counterpart && listed(spec) ? `${field}-sentence` : '',
              refused.length > 0 ? `${field}-issue` : '',
            ]
              .filter(Boolean)
              .join(' ') || undefined}
            invalid={refused.length > 0}
            value={condition.value}
            suggestions={facetOf(named, spec?.suggestions)}
            suggestionsLoading={facetsLoading}
            suggestionsError={facetsError}
            {scales}
            onChange={(value, unreadable) => onUpdate(index, value, unreadable)}
          />
          <FieldIssues {field} issues={refused} />
        </div>
        <button
          id="rules-{list}-{index}-delete"
          type="button"
          class="btn btn-secondary btn-sm"
          onclick={() => remove(index)}
          aria-label="{t('Delete')} – {spec?.label ?? condition.type}"
          title={t('Delete')}
        >
          <Trash2 size={14} />
        </button>
      </div>
    {/each}
  </div>

  {#snippet adder()}
    <Plus size={14} aria-hidden="true" />
    {t('AddCondition')}
  {/snippet}
  <div class="mt-2">
    <ActionMenu
      id="rules-{list}-add"
      label="{t('AddCondition')} – {title}"
      trigger={adder}
      align="start"
      describedBy={adding.length > 0 ? `rules-${list}-add-issue` : undefined}
      actions={choices}
    />
  </div>
  <FieldIssues field="rules-{list}-add" issues={adding} />
</div>
