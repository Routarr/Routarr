<script lang="ts">
  import type { Snippet } from 'svelte';

  /**
   * Nothing here yet. It offers no action: the header already holds that button, and a second
   * control of the same name is ambiguous to a screen reader.
   */
  let { children }: { children: Snippet } = $props();
</script>

<div class="empty-state">{@render children()}</div>
