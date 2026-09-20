<script lang="ts">
  import { CheckCircle2 } from '../lib/icons';

  let { message }: { message: string | null } = $props();
</script>

{#if message}
  <div class="banner banner-success" role="status">
    <CheckCircle2 size={16} />
    <span>{message}</span>
  </div>
{/if}
