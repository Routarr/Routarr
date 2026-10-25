<script lang="ts">
  import { AlertTriangle } from '../lib/icons';
  import { t } from '../lib/i18n.svelte';

  let {
    message,
    id,
    onDismiss,
    onRetry,
    onReload,
  }: {
    message: string | null;
    /** The message's id, for the field the failure is about to name as its description. */
    id?: string;
    onDismiss?: () => void;
    /** For a load, which is safe to make again, never for a write. */
    onRetry?: () => void;
    /** For code that never arrived, which a reload of the page fetches. */
    onReload?: () => void;
  } = $props();
</script>

{#if message}
  <!-- `alert`: a failure is announced without waiting for focus to reach it. -->
  <div class="banner banner-danger" role="alert">
    <AlertTriangle size={16} />
    <span class="flex-1" {id}>{message}</span>
    {#if onRetry}
      <button class="btn btn-secondary btn-sm" onclick={onRetry}>{t('Retry')}</button>
    {/if}
    {#if onReload}
      <button class="btn btn-secondary btn-sm" onclick={onReload}>{t('ReloadPage')}</button>
    {/if}
    {#if onDismiss}
      <button class="btn btn-secondary btn-sm" onclick={onDismiss}>{t('Dismiss')}</button>
    {/if}
  </div>
{/if}
