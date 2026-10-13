<script lang="ts">
  import { AlertTriangle } from '../lib/icons';
  import { t } from '../lib/i18n.svelte';

  /**
   * Stands in for a screen that threw or whose code never arrived. A reload fetches the current
   * code, which a tab left open across an update needs.
   */
  let { error }: { error: unknown } = $props();
</script>

<!-- `alert`: a failure is announced without waiting for focus to reach it. -->
<div class="banner banner-danger items-start" role="alert">
  <AlertTriangle size={16} />
  <div class="flex-1">
    <strong>{t('UnexpectedError')}</strong>
    <!-- The raw message is what makes a bug report actionable. -->
    <div class="mono text-sm mt-2">
      {error instanceof Error ? error.message : String(error)}
    </div>
    <div class="flex gap-2 mt-4">
      <button class="btn btn-secondary btn-sm" onclick={() => location.reload()}>
        {t('ReloadPage')}
      </button>
    </div>
  </div>
</div>
