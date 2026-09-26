<script lang="ts">
  import GettingStarted from '../components/GettingStarted.svelte';
  import { onboarding } from '../lib/onboarding.svelte';
  import type { Outcome } from '../lib/outcome.svelte';

  /**
   * The guide as the dashboard wires it: drawn from the published state, so a
   * choice the server accepts replaces the block that held the pressed button,
   * inside the content region the shell names `main`.
   */
  let { outcome }: { outcome: Outcome } = $props();
</script>

<main id="main" tabindex="-1">
  {#if onboarding.current}
    <GettingStarted status={onboarding.current} {outcome} />
  {/if}
</main>
