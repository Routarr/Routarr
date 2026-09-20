<script lang="ts">
  import { Info } from '../lib/icons';

  let { message }: { message: string } = $props();
</script>

<div class="banner banner-warning" role="status">
  <Info size={16} />
  <span>{message}</span>
</div>
