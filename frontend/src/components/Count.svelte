<script lang="ts">
  import { formatCount } from '../api/format';
  import { i18n } from '../lib/i18n.svelte';

  /**
   * A count in a table cell, in the shape `.num` gives every count: a number,
   * not a verdict. Zero mapped folders, the one worth a warning, is said in
   * the warnings of the screen that shows it, not in this cell's colour.
   */
  let { value }: { value: number } = $props();
</script>

<span class="num">{formatCount(value, i18n.language)}</span>
