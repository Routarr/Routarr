<script lang="ts">
  import { Plus, Trash2 } from '../lib/icons';
  import type { Condition, ConditionSpec, LibraryFacets } from '../api/types';
  import { i18n, t } from '../lib/i18n.svelte';
  import { facetOf, localFacets } from '../api/conditions';
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
     * Every spec, for rendering what the rule already carries. Never filtered:
     * a rule saved before its media type narrowed still has to display, and a
     * condition with no spec found renders as nothing at all.
     */
    specs: ConditionSpec[];
    /** The subset offerable for this rule's media type. Only the add picker. */
    addable: ConditionSpec[];
    /**
     * What the library holds per axis, so a condition can offer its values
     * instead of asking for an exact spelling. Absent while it loads.
     */
    facets?: LibraryFacets | null;
    facetsLoading?: boolean;
    facetsError?: string | null;
    onAdd: (type: string) => void;
    /**
     * Swap a condition for the one asking the same question with the other
     * quantifier, keeping its values.
     */
    onRetype: (index: number, type: string) => void;
    /** `unreadable` when the field holds text `value` could not take in. */
    onUpdate: (index: number, value: unknown, unreadable: boolean) => void;
    onRemove: (index: number) => void;
  } = $props();

  const named = $derived(facets ? localFacets(facets, i18n.language) : null);

  // A quantified pair is one question, so the row is captioned by the `any`
  // half of it and the selector carries the difference. Captioning each half
  // with its own label would say "all of" twice, once in words and once in a
  // control.
  function caption(spec?: ConditionSpec): string | undefined {
    if (spec?.quantifier !== 'all') return spec?.label;
    return specs.find((candidate) => candidate.type === spec.counterpart)?.label ?? spec.label;
  }

  // Only one half of each pair is offered: the other is a turn of the selector,
  // and listing both would put two entries that read alike in one picker.
  const offerable = $derived(addable.filter((spec) => spec.quantifier !== 'all'));

  /**
   * The condition picked to add. On Windows and Linux an arrow key on a closed
   * select fires `change`, so adding on `change` would add a condition at each
   * arrow. Picking is free, and Add adds.
   */
  let chosen = $state('');

  function addChosen() {
    if (!chosen) return;
    onAdd(chosen);
    chosen = '';
    // Add turns disabled with nothing picked, and a disabled button drops the
    // focus: the picker takes it back, ready for the next condition.
    void handFocus(`rules-${list}-add`);
  }

  /**
   * Delete takes its own row away, and can take the focus with it. The
   * condition that moved up into its place takes it, else the picker that adds
   * one, rather than the page behind the dialog.
   */
  function remove(index: number) {
    onRemove(index);
    void handFocus(`rules-${list}-${index}-delete`, `rules-${list}-add`);
  }

  /** The condition kinds whose caption says "not": `genre_not_contains`, `original_language_not`. */
  const NEGATED = /_not(_|$)/;
</script>

<!-- A caption over a *list* of controls is a group heading, not a label: a
     `<label>` must point at exactly one control, and this component renders
     twice (conditions and exclusions), so a fixed `for` would also duplicate
     the id. -->
<div class="form-group" role="group" aria-labelledby="rules-{list}-caption">
  <span class="form-label" id="rules-{list}-caption">{title}</span>
  <!-- How the values inside a condition combine is the selector's business. The
       match mode above combines the conditions themselves. -->
  {#if list === 'conditions'}
    <p class="text-muted text-sm mt-1">{t('ConditionValuesHelp')}</p>
  {:else}
    <!-- What an exclusion does, under a short caption like the conditions'
         own, rather than in a caption long enough to fold on a phone. -->
    <p class="text-muted text-sm mt-1">{t('ExclusionsHelp')}</p>
  {/if}

  <div class="flex flex-col gap-2 mt-2">
    {#each conditions as condition, index (keys[index] ?? index)}
      {@const spec = specs.find((candidate) => candidate.type === condition.type)}
      <div class="condition-row {list === 'exclusions' ? 'excluded' : ''}">
        <span class="condition-caption min-w-190 text-md">{caption(spec) ?? condition.type}</span>
        {#if spec?.counterpart}
          <select
            class="form-select w-auto"
            aria-label="{t('QuantifierLabel')} – {caption(spec)}"
            value={spec.quantifier}
            onchange={(event) =>
              onRetype(index, event.currentTarget.value === 'all' ? spec.counterpart : spec.type)}
          >
            <option value="any">{t('QuantifierAny')}</option>
            <option value="all">{t('QuantifierAll')}</option>
          </select>
        {:else if spec?.value_type === 'string_list' || spec?.value_type === 'number_list'}
          <!-- No counterpart means the media side holds one value, so several
               values can only be alternatives: said in words where the
               selector would stand, or the search box after the first value
               reads as an invitation to give a title a second language. The
               field names it as its description, so it is heard as well. -->
          <!-- Under a negated caption ("does not contain") the words differ in
               most languages, where English says "any of" either way. -->
          <span class="condition-quantifier" id="rules-{list}-{index}-quantifier"
            >{t(NEGATED.test(condition.type) ? 'QuantifierNoneOf' : 'QuantifierAny')}</span
          >
        {/if}
        <div class="condition-value flex-1">
          <ConditionValue
            {spec}
            describedBy={!spec?.counterpart &&
            (spec?.value_type === 'string_list' || spec?.value_type === 'number_list')
              ? `rules-${list}-${index}-quantifier`
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
      {#each offerable as spec (spec.type)}
        <!-- Flagged only when no enabled source can answer it: with the Arr as
             a source, a genre condition needs no warning at all. -->
        <option value={spec.type}>
          {spec.label}{spec.available ? '' : ` ${t('NeedsMetadataSuffix')}`}
        </option>
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
