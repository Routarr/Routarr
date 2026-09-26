<script lang="ts">
  import { t } from '../lib/i18n.svelte';

  /**
   * Whether an instance answers, as a badge, the way every status in a table
   * is drawn, on the dashboard and on Diagnostics alike.
   *
   * `unchecked` is a probe that has not answered yet: saying so beats showing
   * a state nobody knows, and beats an empty cell read as "no instance". A
   * disabled instance is not a failure, so it takes no alarm colour.
   */
  let { status }: { status: string } = $props();
</script>

{#if status === 'unchecked'}
  <span class="badge badge-value muted">{t('Checking')}</span>
{:else if status === 'connected'}
  <span class="badge badge-success">{t('Connected')}</span>
{:else if status === 'disabled'}
  <span class="badge badge-value muted">{t('Disabled')}</span>
{:else}
  <span class="badge badge-danger" title={status}>{status}</span>
{/if}
