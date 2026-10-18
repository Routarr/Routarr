<script lang="ts">
  import { Plus, Trash2 } from '../lib/icons';
  import type { Condition, ConditionSpec, LibraryFacets } from '../api/types';
  import { i18n, t } from '../lib/i18n.svelte';
  import { facetOf, localFacets } from '../api/conditions';
  import { phraseKey } from '../api/format';
  import { handFocus } from '../lib/focus';
  import ConditionValue from './ConditionValue.svelte';

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
    /** The specs the add picker offers for this rule's media type. */
    addable: ConditionSpec[];
    /** The library's values per axis, offered as suggestions. Null while they load. */
    facets?: LibraryFacets | null;
    facetsLoading?: boolean;
    facetsError?: string | null;
    onAdd: (type: string) => void;
    /** Swap a condition for its counterpart with the other quantifier, keeping its values. */
    onRetype: (index: number, type: string) => void;
    /** `unreadable` when the field holds text `value` could not take in. */
    onUpdate: (index: number, value: unknown, unreadable: boolean) => void;
    onRemove: (index: number) => void;
  } = $props();

  const named = $derived(facets ? localFacets(facets, i18n.language) : null);

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
   * The condition picked to add. Adding waits for the button: on Windows and Linux an arrow
   * key on a closed select fires `change`.
   */
  let chosen = $state('');

  function addChosen() {
    if (!chosen) return;
    onAdd(chosen);
    chosen = '';
    // Add turns disabled with nothing picked, and a disabled button drops the focus.
    void handFocus(`rules-${list}-add`);
  }

  /** The focus goes to the row that moves up into the deleted one's place, else the picker. */
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
  <span class="form-label" id="rules-{list}-caption">{title}</span>
  {#if list === 'conditions'}
    <p class="text-muted text-sm mt-1">{t('ConditionValuesHelp')}</p>
  {:else}
    <!-- Explained here so the caption stays short enough not to fold on a phone. -->
    <p class="text-muted text-sm mt-1">{t('ExclusionsHelp')}</p>
  {/if}

  <div class="flex flex-col gap-2 mt-2">
    {#each conditions as condition, index (keys[index] ?? index)}
      {@const spec = specs.find((candidate) => candidate.type === condition.type)}
      <div class="condition-row {list === 'exclusions' ? 'excluded' : ''}">
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
            describedBy={!spec?.counterpart && listed(spec)
              ? `rules-${list}-${index}-sentence`
              : undefined}
            value={condition.value}
            suggestions={facetOf(named, spec?.suggestions)}
            suggestionsLoading={facetsLoading}
            suggestionsError={facetsError}
            onChange={(value, unreadable) => onUpdate(index, value, unreadable)}
          />
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

  <div class="flex gap-2 mt-2">
    <select
      id="rules-{list}-add"
      class="form-select"
      aria-label={title}
      value={chosen}
      onchange={(event) => (chosen = event.currentTarget.value)}
    >
      <option value="">{t('AddCondition')}</option>
      {#each families as family (family.group)}
        <optgroup label={t(FAMILIES[family.group])}>
          {#each family.specs as spec (spec.type)}
            <!-- Flagged only when no enabled source, the Arr included, can answer it. -->
            <option value={spec.type}>
              {spec.label}{spec.available ? '' : ` ${t('NeedsMetadataSuffix')}`}
            </option>
          {/each}
        </optgroup>
      {/each}
    </select>
    <button
      type="button"
      class="btn btn-secondary"
      disabled={!chosen}
      aria-label="{t('Add')} – {title}"
      onclick={addChosen}
    >
      <Plus size={14} />
      {t('Add')}
    </button>
  </div>
</div>
