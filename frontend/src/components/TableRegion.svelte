<script lang="ts">
  import type { Snippet } from 'svelte';

  /**
   * The scrollable box around a table: focusable, so a keyboard reaches what is off-screen
   * (axe's `scrollable-region-focusable`), and named by the table's caption.
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
