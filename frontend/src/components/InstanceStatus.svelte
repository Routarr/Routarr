<script lang="ts">
  import { t } from '../lib/i18n.svelte';

  /**
   * Whether an instance answers, as a badge, on the dashboard and on Diagnostics. `unchecked` is
   * a probe not answered yet, `unknown` one that failed. A disabled instance takes no alarm
   * colour. A failure writes out `detail`, what to change, in the reader's language.
   */
  let { status, detail = null }: { status: string; detail?: string | null } = $props();
</script>

{#if status === 'unchecked'}
  <span class="badge badge-value muted">{t('Checking')}</span>
{:else if status === 'unknown'}
  <span class="badge badge-value muted">{t('Unknown')}</span>
{:else if status === 'connected'}
  <span class="badge badge-success">{t('Connected')}</span>
{:else if status === 'disabled'}
  <span class="badge badge-value muted">{t('InstanceDisabled')}</span>
{:else}
  <span class="badge badge-danger">{t('Error')}</span>
  {#if detail}<span class="status-reason">{detail}</span>{/if}
{/if}
