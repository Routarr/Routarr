<script lang="ts">
  import { formatCount } from '../api/format';
  import { i18n } from '../lib/i18n.svelte';

  /**
   * A count in a table cell, never coloured: a zero worth a warning is raised among the
   * screen's warnings.
   */
  let { value }: { value: number } = $props();
</script>

<span class="num">{formatCount(value, i18n.language)}</span>
