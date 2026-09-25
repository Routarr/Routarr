<script lang="ts">
  import { X } from '../lib/icons';
  import type { Facet } from '../api/types';
  import { t } from '../lib/i18n.svelte';
  import { canonicalKey, addValue, removeValue } from '../api/conditions';

  /**
   * Several values for one condition, picked from what the library holds.
   *
   * The values are the strings the engine compares, so typing one by hand means
   * knowing its exact spelling — the list is what removes that. Free entry stays
   * available for a value no source has answered with yet, which is why this is
   * a combo box over a text input rather than a `<select multiple>`.
   */
  let {
    label,
    values,
    options,
    loading = false,
    error = null,
    onChange,
  }: {
    label: string;
    values: string[];
    /// What the library carries on this axis, commonest first. Empty is a
    /// legitimate state: nothing synced yet, or an axis no source answers.
    options: Facet[];
    loading?: boolean;
    error?: string | null;
    onChange: (values: string[]) => void;
  } = $props();

  // Two pickers sit on one form (conditions and exclusions), so the list's id
  // has to be this instance's own: a shared one leaves `aria-controls`
  // pointing at whichever rendered first.
  const listId = $props.id();

  let query = $state('');
  let open = $state(false);
  let root: HTMLDivElement | undefined = $state();

  // Compared on the canonical key, so an option already chosen under another
  // spelling is not offered a second time.
  const chosen = $derived(new Set(values.map(canonicalKey)));
  // Matched on the label as well as the value: a language is stored as `ja` and
  // searched for as "japanese".
  const matches = $derived(
    options
      .filter((option) => !chosen.has(canonicalKey(option.value)))
      .filter((option) =>
        `${canonicalKey(option.value)} ${canonicalKey(option.label ?? '')}`.includes(
          canonicalKey(query),
        ),
      )
      .slice(0, 50),
  );

  /// What a stored value is called, so a chip reads "Japanese (ja)" and not `ja`.
  const shown = (value: string) =>
    options.find((option) => canonicalKey(option.value) === canonicalKey(value))?.label ?? value;

  // The typed text is offerable in its own right only when no option already
  // carries it: otherwise picking it would store a spelling the library does
  // not use, which reads as a duplicate in the chip row.
  const custom = $derived(
    query.trim() &&
      !chosen.has(canonicalKey(query)) &&
      !options.some((option) => canonicalKey(option.value) === canonicalKey(query))
      ? query.trim()
      : '',
  );

  function add(value: string) {
    onChange(addValue(values, value));
    query = '';
  }

  function onKey(event: KeyboardEvent) {
    if (event.key === 'Escape') {
      open = false;
      return;
    }
    // A comma is a separator everywhere else in this form, so it commits here
    // too rather than landing in a value nothing will ever match.
    if (event.key === 'Enter' || event.key === ',') {
      event.preventDefault();
      // The list wins over the text: typing part of a value and pressing Enter
      // has to store the value, not the fragment. The typed text is only taken
      // when the library offers nothing like it.
      const first = matches[0];
      if (first) add(first.value);
      else if (custom) add(custom);
    }
  }
</script>

<div
  class="value-picker"
  bind:this={root}
  onfocusout={(event) => {
    if (!root?.contains(event.relatedTarget as Node)) open = false;
  }}
>
  {#if values.length}
    <ul class="chips" aria-label={t('SelectedValues')}>
      {#each values as value (value)}
        <li class="chip">
          <span>{shown(value)}</span>
          <button
            type="button"
            class="chip-remove"
            aria-label="{t('Remove')} – {shown(value)}"
            onclick={() => onChange(removeValue(values, value))}
          >
            <X size={12} />
          </button>
        </li>
      {/each}
    </ul>
  {/if}

  <input
    aria-label={label}
    class="form-input"
    role="combobox"
    aria-expanded={open}
    aria-autocomplete="list"
    aria-controls={listId}
    autocomplete="off"
    placeholder={t('PlaceholderPickValues')}
    value={query}
    oninput={(event) => {
      query = event.currentTarget.value;
      open = true;
    }}
    onfocus={() => (open = true)}
    onkeydown={onKey}
  />

  {#if open}
    <!-- Unnamed on purpose: `aria-controls` already ties it to the combobox,
         and repeating that name gives two elements one label. -->
    <div class="picker-list" id={listId} role="listbox">
      {#if loading}
        <p class="picker-note">{t('Loading')}</p>
      {:else if error}
        <p class="picker-note">{error}</p>
      {:else}
        {#each matches as option (option.value)}
          <button
            type="button"
            class="picker-option"
            role="option"
            aria-selected="false"
            aria-label={option.label ?? option.value}
            onclick={() => add(option.value)}
          >
            <span>{option.label ?? option.value}</span>
            <!-- A vocabulary entry is not an observation, so it carries no
                 figure: a 0 beside it would read as "absent from the library"
                 where the point is that it can be chosen anyway. -->
            {#if option.count > 0}<span class="picker-count">{option.count}</span>{/if}
          </button>
        {/each}
        {#if custom}
          <button
            type="button"
            class="picker-option"
            role="option"
            aria-selected="false"
            onclick={() => add(custom)}
          >
            {t('AddValue', { value: custom })}
          </button>
        {/if}
        {#if !matches.length && !custom}
          <p class="picker-note">{options.length ? t('NoMatchingValue') : t('NoValueInLibrary')}</p>
        {/if}
      {/if}
    </div>
  {/if}
</div>
