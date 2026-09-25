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
{#if outcome.warning}
  <WarningBanner message={outcome.warning} />
{/if}
<SuccessBanner message={outcome.notice} />
{#each outcome.details as detail, index (index)}
  <ErrorBanner message={detail} />
{/each}
