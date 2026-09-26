<script lang="ts">
  import type { Outcome } from '../lib/outcome.svelte';
  import ErrorBanner from './ErrorBanner.svelte';
  import SuccessBanner from './SuccessBanner.svelte';
  import WarningBanner from './WarningBanner.svelte';

  /**
   * The one way a screen shows how an action went. The error offers no Retry:
   * trying a write again is the operator's decision, and a reload of the page
   * would not replay it.
   */
  let { outcome }: { outcome: Outcome } = $props();
</script>

<ErrorBanner message={outcome.error} onDismiss={() => outcome.clear()} />
<!-- Mounted before anything is said in it: a live region that arrives with its
     text announces nothing, and a success is not an alert. -->
<div role="status">
  {#if outcome.warning}
    <WarningBanner message={outcome.warning} />
  {:else if outcome.notice}
    <SuccessBanner message={outcome.notice} />
  {/if}
</div>
{#each outcome.details as detail, index (index)}
  <ErrorBanner message={detail} />
{/each}
