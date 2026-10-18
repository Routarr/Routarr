<script lang="ts">
  import { Info } from '../lib/icons';
  import { t } from '../lib/i18n.svelte';

  /** No live region of its own, since one nested in `OutcomeBanner`'s can be read twice. */
  let {
    message,
    onRetry,
    action,
  }: {
    message: string;
    /** For a part of a screen that failed to load while the rest stands. */
    onRetry?: () => void;
    /** The one step the warning leads to, named by its button. */
    action?: { label: string; run: () => void };
  } = $props();
</script>

<div class="banner banner-warning">
  <Info size={16} />
  <span class="flex-1">{message}</span>
  {#if onRetry}
    <button class="btn btn-secondary btn-sm" onclick={onRetry}>{t('Retry')}</button>
  {/if}
  {#if action}
    <button class="btn btn-secondary btn-sm" onclick={action.run}
      ><span>{action.label}</span></button
    >
  {/if}
</div>
