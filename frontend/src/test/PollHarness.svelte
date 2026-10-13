<script lang="ts">
  import { poll } from '../lib/poll.svelte';

  /** `poll` opens an effect, so it runs inside a component. */
  let { reload, every, active }: { reload: () => void; every: number; active: boolean } = $props();

  poll(
    () => reload(),
    () => every,
    () => active,
  );
</script>
