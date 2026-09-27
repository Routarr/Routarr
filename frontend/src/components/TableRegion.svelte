<script lang="ts">
  import type { Snippet } from 'svelte';

  /**
   * The scrollable box around a table, named and reachable.
   *
   * A table wider than its window scrolls inside this box, and a box that
   * scrolls has to be focusable or a keyboard never reaches what is off-screen —
   * axe's `scrollable-region-focusable`. Svelte's `a11y_no_noninteractive_tabindex`
   * flags the same attribute, since a focusable region must be named: it is,
   * by the caption of the table it holds. One component carries both the
   * attributes and the one place the two rules have to be reconciled.
   */
  let {
    label,
    id,
    class: extra = '',
    children,
  }: {
    label: string;
    /** For a screen that hands the focus here once the rows it held are gone. */
    id?: string;
    class?: string;
    children: Snippet;
  } = $props();
</script>

<!-- svelte-ignore a11y_no_noninteractive_tabindex -->
<div {id} class="table-container {extra}" role="region" aria-label={label} tabindex="0">
  {@render children()}
</div>
