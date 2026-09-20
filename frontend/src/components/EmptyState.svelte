<script lang="ts">
  import type { Snippet } from 'svelte';

  /**
   * Nothing here yet.
   *
   * Deliberately without the action that fills it, which is the right instinct
   * in general and wrong on these screens: the header already offers that button
   * permanently, and repeating it puts two controls with the same name on one
   * page — ambiguous for a screen reader before it is ambiguous for a test.
   */
  let { children }: { children: Snippet } = $props();
</script>

<div class="empty-state">{@render children()}</div>
