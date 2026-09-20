<script lang="ts">
  import { Loader2 } from '../lib/icons';
  import { t } from '../lib/i18n.svelte';

  let { label }: { label?: string } = $props();
</script>

<div class="empty-state">
  <Loader2 size={20} class="spin" />
  <span>{label ?? t('Loading')}</span>
</div>
