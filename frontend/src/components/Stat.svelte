<script lang="ts">
  /** One figure and its name, on a strip of figures. */
  import { formatCount } from '../api/format';
  import { i18n } from '../lib/i18n.svelte';

  let { label, value, tone }: { label: string; value: number; tone?: string } = $props();

  // A zero in the danger colour would read as a failure.
  const shown = $derived(tone && value !== 0 ? tone : undefined);
</script>

<div class="metric{shown ? ` is-${shown}` : ''}">
  <span class="metric-value">{formatCount(value, i18n.language)}</span>
  <span class="metric-label">{label}</span>
</div>
