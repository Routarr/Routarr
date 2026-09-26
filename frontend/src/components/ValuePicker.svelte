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
    describedBy,
    onChange,
  }: {
    label: string;
    values: string[];
    /// What the library carries on this axis, commonest first. Empty is a
    /// legitimate state: nothing synced yet, or an axis no source answers.
    options: Facet[];
    loading?: boolean;
    error?: string | null;
    /// The element that qualifies the field, as the fixed "any of" beside it.
    describedBy?: string;
    onChange: (values: string[]) => void;
  } = $props();

  // Two pickers sit on one form (conditions and exclusions), so the list's id
  // has to be this instance's own: a shared one leaves `aria-controls`
  // pointing at whichever rendered first.
  const listId = $props.id();

  let query = $state('');
  let open = $state(false);
  let root: HTMLDivElement | undefined = $state();
  /// The suggestion the arrows reached, or none until an arrow is pressed.
  let cursor = $state(-1);

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

  /**
   * The matches in runs of one meaning, in the order the server sends them: a
   * run with a group is drawn under its meaning, a run without is drawn bare.
   * Each keeps its place in the list, which is what the arrows count.
   */
  const blocks = $derived.by(() => {
    const runs: { group?: string; options: { option: Facet; index: number }[] }[] = [];
    matches.forEach((option, index) => {
      const last = runs.at(-1);
      if (last && last.group === option.group) last.options.push({ option, index });
      else runs.push({ group: option.group, options: [{ option, index }] });
    });
    return runs;
  });
  const optionId = (index: number) => `${listId}-${index}`;
  /// Whether the figures beside the values are counts worth a caption.
  const counted = $derived(matches.some((option) => option.count > 0));

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
  /// Every value the list offers, in its order: the matches, then the typed text.
  const choices = $derived([...matches.map((option) => option.value), ...(custom ? [custom] : [])]);

  function add(value: string) {
    onChange(addValue(values, value));
    query = '';
    cursor = -1;
  }

  // The combobox pattern the command palette follows: the focus stays in the
  // field, the arrows move through the list and `aria-activedescendant` says
  // where they are, so the list is one tab stop however long it is.
  function onKey(event: KeyboardEvent) {
    if (event.key === 'Escape') {
      // Claimed only while the list is open. Inside the rule editor's dialog,
      // an Escape nobody claims closes the dialog, which is the next one's.
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
    // A comma is a separator everywhere else in this form, so it commits here
    // too rather than landing in a value nothing will ever match.
    if (event.key === 'Enter' || event.key === ',') {
      const reached = cursor >= 0 ? choices[cursor] : undefined;
      // Nothing typed and nothing reached: Enter is the form's, which saves.
      if (!reached && !query.trim()) {
        if (event.key === ',') event.preventDefault();
        return;
      }
      event.preventDefault();
      // The list wins over the text: typing part of a value and pressing Enter
      // has to store the value, not the fragment. The typed text is only taken
      // when the library offers nothing like it.
      const value = reached ?? matches[0]?.value ?? custom;
      if (value) add(value);
    }
  }
</script>

{#snippet row(option: Facet, index: number, grouped: boolean)}
  <!-- An option carries the click itself rather than wrapping a button, which
       would be a tab stop: the keyboard path is the field's own, as in the
       command palette, so Svelte's rule cannot see the handler it asks for. -->
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
    <!-- Under its meaning a code needs no repeat of it; the name stays on the
         chip once chosen, where no heading stands above it. -->
    <span>{grouped ? option.value : (option.label ?? option.value)}</span>
    <!-- A vocabulary entry is not an observation, so it carries no figure: a
         0 beside it would read as "absent from the library" where the point
         is that it can be chosen anyway. -->
    {#if option.count > 0}<span class="picker-count">{option.count}</span>{/if}
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
    aria-activedescendant={open && cursor >= 0 ? optionId(cursor) : undefined}
    aria-describedby={describedBy}
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

  {#if open}
    <!-- Unnamed on purpose: `aria-controls` already ties it to the combobox,
         and repeating that name gives two elements one label. A press on it
         keeps the focus in the field, where the keyboard path is. -->
    <div
      class="picker-list"
      id={listId}
      role="listbox"
      tabindex="-1"
      onmousedown={(event) => event.preventDefault()}
    >
      {#if loading}
        <p class="picker-note">{t('Loading')}</p>
      {:else if error}
        <p class="picker-note">{error}</p>
      {:else}
        {#if counted}
          <!-- What the figures count, said once above them. -->
          <p class="picker-head" aria-hidden="true">{t('PickerCountCaption')}</p>
        {/if}
        {#each blocks as block, index (index)}
          {#if block.group}
            <div role="group" aria-label={block.group}>
              <p class="picker-group-title" aria-hidden="true">{block.group}</p>
              {#each block.options as entry (entry.option.value)}
                {@render row(entry.option, entry.index, true)}
              {/each}
            </div>
          {:else}
            {#each block.options as entry (entry.option.value)}
              {@render row(entry.option, entry.index, false)}
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
        {#if !matches.length && !custom}
          <p class="picker-note">{options.length ? t('NoMatchingValue') : t('NoValueInLibrary')}</p>
        {/if}
      {/if}
    </div>
  {/if}
</div>
