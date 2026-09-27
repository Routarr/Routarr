<script lang="ts">
  /**
   * One figure and its name, on a line of figures.
   *
   * Eight small integers were eight bordered cards on a grid: 318px of a
   * 1024px screen for Diagnostics, three rows deep with a hole in the last
   * one, and the number set at 26px so it read as the point of the page when
   * it is the preamble to it. The dashboard already stated the same figures as
   * one dense strip; this is that strip, so the two screens stop disagreeing
   * about what a count looks like.
   */
  let { label, value, tone }: { label: string; value: number | string; tone?: string } = $props();

  // Nothing failed is not a failure: a zero in the danger colour reads as one,
  // so the tone is the figure's only while there is something to count.
  const shown = $derived(tone && Number(value) !== 0 ? tone : undefined);
</script>

<div class="metric{shown ? ` is-${shown}` : ''}">
  <span class="metric-value">{value}</span>
  <span class="metric-label">{label}</span>
</div>
