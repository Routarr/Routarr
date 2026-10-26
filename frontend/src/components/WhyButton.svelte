<script lang="ts">
  import { RefreshCw } from '../lib/icons';
  import type { Explainer } from '../lib/explain.svelte';
  import { t } from '../lib/i18n.svelte';

  /** "Why?" after a title the library holds, which opens its explanation. */
  let {
    explainer,
    mediaId,
    title,
  }: {
    explainer: Explainer;
    mediaId: string;
    /** The title the row names, which the button's name repeats so no two rows share it. */
    title: string;
  } = $props();

  const busy = $derived(explainer.asked === mediaId);
</script>

<button
  type="button"
  class="btn btn-secondary btn-sm"
  onclick={() => void explainer.open(mediaId)}
  aria-label="{t('WhyQuestion')} – {title}"
  aria-busy={busy}
>
  {#if busy}
    <RefreshCw size={12} class="spin" aria-hidden="true" />
  {/if}
  {t('WhyQuestion')}
</button>
