<script lang="ts">
  import type { Snippet } from 'svelte';
  import { SlidersHorizontal } from '../lib/icons';
  import { t } from '../lib/i18n.svelte';

  /**
   * A list's search, and its other filters folded behind one button that counts those set:
   * a row of every select reads as clutter on a desktop and runs rows deep on a phone.
   */
  let {
    active,
    clearable,
    onClear,
    views,
    search,
    children,
  }: {
    /** The filters set in the panel, which the button counts. */
    active: number;
    /** Whether anything narrows the list, the search included. */
    clearable: boolean;
    onClear: () => void;
    /** The views a list switches between, before its search: no filter clears them. */
    views?: Snippet;
    search: Snippet;
    /** The panel's labelled fields. */
    children: Snippet;
  } = $props();

  let open = $state(false);
  const panel = $props.id();
</script>

<div class="toolbar">
  {@render views?.()}
  {@render search()}
  <button
    type="button"
    class="btn btn-secondary"
    aria-expanded={open}
    aria-controls={panel}
    onclick={() => (open = !open)}
  >
    <SlidersHorizontal size={16} aria-hidden="true" />
    {t('Filters')}
    {#if active > 0}
      <span class="badge badge-info">{active}</span>
    {/if}
  </button>
  {#if clearable}
    <button type="button" class="btn btn-ghost" onclick={onClear}>{t('ClearFilters')}</button>
  {/if}
</div>
<div id={panel} class="card filter-panel" hidden={!open}>
  <div class="form-row">
    {@render children()}
  </div>
</div>
