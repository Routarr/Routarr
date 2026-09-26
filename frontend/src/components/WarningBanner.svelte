<script lang="ts">
  import { Info } from '../lib/icons';

  /**
   * No live region of its own: inside `OutcomeBanner` it is said through the
   * region that holds it, and a region nested in another can be read twice.
   */
  let { message }: { message: string } = $props();
</script>

<div class="banner banner-warning">
  <Info size={16} />
  <span>{message}</span>
</div>
