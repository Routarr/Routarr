<script lang="ts">
  import type { Outcome } from '../lib/outcome.svelte';
  import BannerList from './BannerList.svelte';
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
<!-- One banner for every item that failed, outside the live regions: the
     headline above is what is announced, and forty alerts read one after the
     other would bury it. -->
<BannerList tone="danger" items={outcome.details.map((text) => ({ text }))} />
