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
</script>

<div class="metric{tone ? ` is-${tone}` : ''}">
  <span class="metric-value">{value}</span>
  <span class="metric-label">{label}</span>
</div>
