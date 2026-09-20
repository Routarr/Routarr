<script lang="ts">
  import { poll } from '../lib/poll.svelte';

  /**
   * `poll` opens an effect, so it can only run inside a component. This is the
   * smallest one that gives it a place to live.
   */
  let { reload, every, active }: { reload: () => void; every: number; active: boolean } = $props();

  poll(
    () => reload(),
    () => every,
    () => active,
  );
</script>
