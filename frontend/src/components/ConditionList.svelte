<script lang="ts">
  import { Trash2 } from '../lib/icons';
  import type {
    Condition,
    ConditionSpec,
    Facet,
    FacetAxis,
    LibraryFacets,
    Vocabularies,
  } from '../api/types';
  import { t } from '../lib/i18n.svelte';
  import { canonicalKey, nameFacets } from '../api/conditions';
  import ConditionValue from './ConditionValue.svelte';

  let {
    title,
    list,
    conditions,
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
    /// Every spec, for rendering what the rule already carries. Never filtered:
    /// a rule saved before its media type narrowed still has to display, and a
    /// condition with no spec found renders as nothing at all.
    specs: ConditionSpec[];
    /// The subset offerable for this rule's media type. Only the add picker.
    addable: ConditionSpec[];
    /// What the library holds per axis, so a condition can offer its values
    /// instead of asking for an exact spelling. Absent while it loads.
    facets?: LibraryFacets | null;
    facetsLoading?: boolean;
    facetsError?: string | null;
    onAdd: (type: string) => void;
    /// Swap a condition for the one asking the same question with the other
    /// quantifier, keeping its values.
    onRetype: (index: number, type: string) => void;
    onUpdate: (index: number, value: unknown) => void;
    onRemove: (index: number) => void;
  } = $props();

  // The catalogue names the axis; this reads it off the payload. Indexed rather
  // than switched on the condition kind, so a condition added in Rust needs no
  // change here.
  function facetOf(spec?: ConditionSpec): Facet[] {
    if (!spec?.suggestions || !facets) return [];
    // Narrowed, not widened: the name arrives from the backend so this is an
    // assertion either way, but `Record<string, Facet[]>` erased every later
    // check as well. A test vouches for the name itself.
    const axis = spec.suggestions as FacetAxis;
    const held = facets[axis] ?? [];
    // A closed vocabulary is offered whole, the library's own values first so
    // the common answer stays at the top. Without it a language rule offers the
    // five codes that happen to be synced, and the other forty-eight have to be
    // guessed — as codes, which nobody would.
    // Only two axes have a closed vocabulary, so this indexing is partial by
    // design and the key may legitimately miss.
    const vocabulary = facets.vocabularies[axis as keyof Vocabularies] ?? [];
    if (!vocabulary.length) return held;

    const seen = new Set(held.map((facet) => canonicalKey(facet.value)));
    return [
      ...nameFacets(held, vocabulary),
      ...vocabulary.filter((facet) => !seen.has(canonicalKey(facet.value))),
    ];
  }

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
</script>

<!-- A caption over a *list* of controls is a group heading, not a label: a
     `<label>` must point at exactly one control, and this component renders
     twice (conditions and exclusions), so a fixed `for` would also have
     duplicated the id. -->
<div class="form-group" role="group" aria-labelledby="rules-{list}-caption">
  <span class="form-label" id="rules-{list}-caption">{title}</span>
  <!-- How the values inside a condition combine is the selector's business; the
       match mode above combines the conditions themselves. -->
  {#if list === 'conditions'}
    <p class="text-muted text-sm mt-1">{t('ConditionValuesHelp')}</p>
  {/if}

  <div class="flex flex-col gap-2 mt-2">
    {#each conditions as condition, index (index)}
      {@const spec = specs.find((candidate) => candidate.type === condition.type)}
      <div class="condition-row {list === 'exclusions' ? 'excluded' : ''}">
        <span class="min-w-190 text-md">{caption(spec) ?? condition.type}</span>
        {#if spec?.counterpart}
          <select
            class="form-select w-auto"
            aria-label="{t('QuantifierLabel')} — {caption(spec)}"
            value={spec.quantifier}
            onchange={(event) =>
              onRetype(index, event.currentTarget.value === 'all' ? spec.counterpart : spec.type)}
          >
            <option value="any">{t('QuantifierAny')}</option>
            <option value="all">{t('QuantifierAll')}</option>
          </select>
        {/if}
        <div class="flex-1">
          <ConditionValue
            {spec}
            value={condition.value}
            suggestions={facetOf(spec)}
            suggestionsLoading={facetsLoading}
            suggestionsError={facetsError}
            onChange={(value) => onUpdate(index, value)}
          />
        </div>
        <button
          type="button"
          class="btn btn-secondary btn-sm"
          onclick={() => onRemove(index)}
          aria-label="{t('Delete')} — {spec?.label ?? condition.type}"
          title={t('Delete')}
        >
          <Trash2 size={14} />
        </button>
      </div>
    {/each}
  </div>

  <select
    class="form-select mt-2"
    aria-label={title}
    value=""
    onchange={(event) => {
      if (event.currentTarget.value) onAdd(event.currentTarget.value);
      event.currentTarget.value = '';
    }}
  >
    <option value="">{t('AddCondition')}</option>
    {#each offerable as spec (spec.type)}
      <!-- Flagged only when no enabled source can answer it: with the Arr as a
           source, a genre condition needs no warning at all. -->
      <option value={spec.type}>
        {spec.label}{spec.available ? '' : ` ${t('NeedsMetadataSuffix')}`}
      </option>
    {/each}
  </select>
</div>
