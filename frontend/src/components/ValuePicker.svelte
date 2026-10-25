<script lang="ts">
  import { X } from '../lib/icons';
  import type { Facet } from '../api/types';
  import { i18n, t } from '../lib/i18n.svelte';
  import { canonicalKey, addValue, removeValue } from '../api/conditions';
  import { formatCount, runsOf } from '../api/format';
  import { handFocus } from '../lib/focus';
  import { composing } from '../lib/keys';

  /**
   * Several values for one condition, picked from what the library holds. A combo box over a
   * text input, so a value no source has answered with yet can still be typed.
   */
  let {
    id,
    label,
    values,
    options,
    loading = false,
    error = null,
    describedBy,
    invalid = false,
    keyOf = canonicalKey,
    onChange,
  }: {
    /** The field's id, for a visible label pointing at it. */
    id?: string;
    label: string;
    values: string[];
    /** What the library carries on this axis, commonest first. Empty until a source answers. */
    options: Facet[];
    loading?: boolean;
    error?: string | null;
    /** What qualifies the field: the fixed "any of" beside it, a refusal under it. */
    describedBy?: string;
    /** Whether the server refused the condition the field holds. */
    invalid?: boolean;
    /** What makes two values one, as the engine compares this condition's. */
    keyOf?: (value: string) => string;
    onChange: (values: string[]) => void;
  } = $props();

  // Per instance: two pickers share a form, and a shared id points aria-controls at the first.
  const listId = $props.id();

  let query = $state('');
  let open = $state(false);
  let root: HTMLDivElement | undefined = $state();
  let field: HTMLInputElement | undefined = $state();
  /** The suggestion the arrows reached, or none until an arrow is pressed. */
  let cursor = $state(-1);

  // Keyed as the engine compares, so a chosen value is not offered again in another spelling.
  const chosen = $derived(new Set(values.map(keyOf)));
  // The label matches too: a language is stored as `ja` and searched for as "japanese".
  const matches = $derived(
    options
      .filter((option) => !chosen.has(keyOf(option.value)))
      .filter((option) =>
        `${canonicalKey(option.value)} ${canonicalKey(option.label ?? '')}`.includes(
          canonicalKey(query),
        ),
      )
      .slice(0, 50),
  );

  /**
   * The matches in runs of one meaning, in the server's order, a run with a group drawn under
   * it. Each keeps its index in the list, which the arrows count.
   */
  const blocks = $derived(runsOf(matches, (option) => option.group));
  const optionId = (index: number) => `${listId}-${index}`;
  /** Whether the figures beside the values are counts worth a caption. */
  const counted = $derived(matches.some((option) => option.count > 0));

  /** What a stored value is called, so a chip reads "Japanese (ja)" and not `ja`. */
  const shown = (value: string) =>
    options.find((option) => keyOf(option.value) === keyOf(value))?.label ?? value;

  // Offered only when no option carries it, or the chip row would show a duplicate spelling.
  const custom = $derived(
    query.trim() &&
      !chosen.has(keyOf(query)) &&
      !options.some((option) => keyOf(option.value) === keyOf(query))
      ? query.trim()
      : '',
  );
  /** Every value the list offers, in its order: the matches, then the typed text. */
  const choices = $derived([...matches.map((option) => option.value), ...(custom ? [custom] : [])]);
  /** What is said in place of the list, which then holds nothing to choose. */
  const note = $derived(
    loading
      ? t('Loading')
      : error
        ? error
        : choices.length === 0
          ? t(options.length ? 'NoMatchingValue' : 'NoValueInLibrary')
          : null,
  );
  const listed = $derived(open && note === null);

  function add(value: string) {
    onChange(addValue(values, value, keyOf));
    query = '';
    cursor = -1;
  }

  const removeId = (index: number) => `${listId}-remove-${index}`;

  /**
   * The focus moves to the chip that takes the removed one's place, else the one before, else
   * the field, so removing several values is one key pressed several times.
   */
  function remove(value: string, index: number) {
    onChange(removeValue(values, value, keyOf));
    void handFocus(removeId(index), removeId(index - 1), field);
  }

  // Combobox pattern: the focus stays in the field and aria-activedescendant follows the arrows.
  function onKey(event: KeyboardEvent) {
    if (composing(event)) return;
    if (event.key === 'Escape') {
      // Claimed only while open: an unclaimed Escape closes the rule editor's dialog.
      if (open) {
        event.preventDefault();
        open = false;
        cursor = -1;
      }
      return;
    }
    if (event.key === 'ArrowDown' || event.key === 'ArrowUp') {
      event.preventDefault();
      open = true;
      if (choices.length === 0) return;
      const step = event.key === 'ArrowDown' ? 1 : -1;
      cursor =
        cursor < 0 && step < 0
          ? choices.length - 1
          : (cursor + step + choices.length) % choices.length;
      // Optional, as the palette's: jsdom ships the element, not the method.
      document.getElementById(optionId(cursor))?.scrollIntoView?.({ block: 'nearest' });
      return;
    }
    // A comma separates values everywhere else in this form, so it commits here too.
    if (event.key === 'Enter' || event.key === ',') {
      const reached = cursor >= 0 ? choices[cursor] : undefined;
      // Nothing typed and nothing reached: Enter is the form's, which saves.
      if (!reached && !query.trim()) {
        if (event.key === ',') event.preventDefault();
        return;
      }
      event.preventDefault();
      // The list wins over the typed fragment, which is kept only when nothing matches.
      const value = reached ?? matches[0]?.value ?? custom;
      if (value) add(value);
    }
  }
</script>

{#snippet row(option: Facet, index: number, grouped: boolean)}
  <!-- A button inside an option would be a tab stop. The keys belong to the field,
       as in the command palette, so Svelte's rule cannot see the handler. -->
  <!-- svelte-ignore a11y_click_events_have_key_events -->
  <div
    id={optionId(index)}
    class="picker-option"
    role="option"
    tabindex="-1"
    aria-selected={index === cursor}
    aria-label={option.label ?? option.value}
    onclick={() => add(option.value)}
  >
    <!-- Under a heading that names it, the code alone. The chip keeps the name. -->
    <span>{grouped ? option.value : (option.label ?? option.value)}</span>
    <!-- A vocabulary entry shows no figure: a 0 would read as absent, yet it can be chosen. -->
    {#if option.count > 0}<span class="picker-count"
        >{formatCount(option.count, i18n.language)}</span
      >{/if}
  </div>
{/snippet}

<div
  class="value-picker"
  bind:this={root}
  onfocusout={(event) => {
    if (!root?.contains(event.relatedTarget as Node)) open = false;
  }}
>
  {#if values.length}
    <ul class="chips" aria-label={t('SelectedValues')}>
      {#each values as value, index (value)}
        <li class="chip">
          <span>{shown(value)}</span>
          <button
            id={removeId(index)}
            type="button"
            class="chip-remove"
            aria-label="{t('Remove')} – {shown(value)}"
            onclick={() => remove(value, index)}
          >
            <X size={12} />
          </button>
        </li>
      {/each}
    </ul>
  {/if}

  <input
    bind:this={field}
    {id}
    aria-label={label}
    class="form-input"
    role="combobox"
    aria-expanded={listed}
    aria-autocomplete="list"
    aria-controls={listId}
    aria-activedescendant={listed && cursor >= 0 ? optionId(cursor) : undefined}
    aria-describedby={describedBy}
    aria-invalid={invalid ? 'true' : undefined}
    autocomplete="off"
    placeholder={t('PlaceholderPickValues')}
    value={query}
    oninput={(event) => {
      query = event.currentTarget.value;
      open = true;
      cursor = -1;
    }}
    onfocus={() => (open = true)}
    onkeydown={onKey}
  />

  {#if listed}
    <!-- Unnamed: aria-controls ties it to the combobox, which carries the name. A press
         on it keeps the focus in the field. -->
    <div
      class="picker-list"
      id={listId}
      role="listbox"
      tabindex="-1"
      onmousedown={(event) => event.preventDefault()}
    >
      {#if counted}
        <!-- What the figures count, said once above them. -->
        <p class="picker-head" aria-hidden="true">{t('PickerCountCaption')}</p>
      {/if}
      {#each blocks as block, index (index)}
        {#if block.key}
          <div role="group" aria-label={block.key}>
            <p class="picker-group-title" aria-hidden="true">{block.key}</p>
            {#each block.entries as entry (entry.item.value)}
              {@render row(entry.item, entry.index, true)}
            {/each}
          </div>
        {:else}
          {#each block.entries as entry (entry.item.value)}
            {@render row(entry.item, entry.index, false)}
          {/each}
        {/if}
      {/each}
      {#if custom}
        <!-- svelte-ignore a11y_click_events_have_key_events -->
        <div
          id={optionId(matches.length)}
          class="picker-option"
          role="option"
          tabindex="-1"
          aria-selected={cursor === matches.length}
          onclick={() => add(custom)}
        >
          {t('AddValue', { value: custom })}
        </div>
      {/if}
    </div>
  {/if}
  <!-- Beside the list, which owns options alone, and mounted with the field so its note is
       heard where the focus stays. Drawn where the list would be. -->
  <div role="status">
    {#if open && note !== null}
      <div class="picker-list">
        <p class="picker-note">{note}</p>
      </div>
    {/if}
  </div>
</div>
