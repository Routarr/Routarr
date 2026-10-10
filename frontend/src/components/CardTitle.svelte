<script lang="ts">
  import type { Snippet } from 'svelte';
  import HelpToggle from './HelpToggle.svelte';

  /**
   * A card's title and the "?" beside it. Every card title of the application
   * is drawn here or beside its own `HelpToggle`, so no card goes without its
   * help: `layout.test.ts` refuses a `card-title` with neither.
   */
  let {
    card,
    level = 2,
    id,
    tabindex,
    class: extra = '',
    children,
  }: {
    /** The card's id in `CARD_HELP`, which holds what its "?" says. */
    card: string;
    level?: 2 | 3;
    /** For a title something else points at, as the guide's focus does. */
    id?: string;
    tabindex?: number;
    class?: string;
    children: Snippet;
  } = $props();

  const uid = $props.id();
  const heading = $derived(id ?? `card-title-${uid}`);
</script>

<div class="card-title-group">
  <svelte:element this={`h${level}`} class="card-title {extra}" id={heading} {tabindex}>
    {@render children()}
  </svelte:element>
  <HelpToggle {card} title={heading} />
</div>
