<script lang="ts">
  import { AlertTriangle } from '../lib/icons';
  import { t } from '../lib/i18n.svelte';

  let {
    message,
    onDismiss,
    onRetry,
  }: {
    message: string | null;
    onDismiss?: () => void;
    /// Offered when the failed request can simply be made again — a load, not
    /// a write — so a network blip does not cost a page reload.
    onRetry?: () => void;
  } = $props();
</script>

{#if message}
  <!-- `alert`: a failure is announced without waiting for focus to reach it. -->
  <div class="banner banner-danger" role="alert">
    <AlertTriangle size={16} />
    <span class="flex-1">{message}</span>
    {#if onRetry}
      <button class="btn btn-secondary btn-sm" onclick={onRetry}>{t('Retry')}</button>
    {/if}
    {#if onDismiss}
      <button class="btn btn-secondary btn-sm" onclick={onDismiss}>{t('Dismiss')}</button>
    {/if}
  </div>
{/if}
