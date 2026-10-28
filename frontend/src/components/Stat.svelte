<script lang="ts">
  /** One figure and its name on a strip of figures, a link to the list it counts if `to` is set. */
  import { formatCount } from '../api/format';
  import { i18n } from '../lib/i18n.svelte';
  import { href } from '../lib/router.svelte';

  let { label, value, tone, to }: { label: string; value: number; tone?: string; to?: string } =
    $props();

  // A zero in the danger colour would read as a failure.
  const shown = $derived(tone && value !== 0 ? tone : undefined);
  const drawn = $derived(`metric${shown ? ` is-${shown}` : ''}`);
</script>

{#snippet figure()}
  <span class="metric-value">{formatCount(value, i18n.language)}</span>
  <span class="metric-label">{label}</span>
{/snippet}

{#if to}
  <a class="{drawn} metric-link" href={href(to)}>{@render figure()}</a>
{:else}
  <div class={drawn}>{@render figure()}</div>
{/if}
