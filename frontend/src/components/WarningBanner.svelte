<script lang="ts">
  import { Info } from '../lib/icons';
  import { t } from '../lib/i18n.svelte';

  /**
   * No live region of its own: inside `OutcomeBanner` it is said through the
   * region that holds it, and a region nested in another can be read twice.
   */
  let {
    message,
    onRetry,
  }: {
    message: string;
    /** For a part of a screen that failed to load while the rest stands. */
    onRetry?: () => void;
  } = $props();
</script>

<div class="banner banner-warning">
  <Info size={16} />
  <span class="flex-1">{message}</span>
  {#if onRetry}
    <button class="btn btn-secondary btn-sm" onclick={onRetry}>{t('Retry')}</button>
  {/if}
</div>
