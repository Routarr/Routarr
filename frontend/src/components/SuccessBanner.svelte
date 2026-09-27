<script lang="ts">
  import { CheckCircle2 } from '../lib/icons';

  /** Said through the live region `OutcomeBanner` holds, so it carries none of its own. */
  let { message }: { message: string } = $props();
</script>

<div class="banner banner-success">
  <CheckCircle2 size={16} />
  <span>{message}</span>
</div>
