<script lang="ts">
  /**
   * One figure and its name, on a line of figures.
   *
   * Small integers read as the preamble to a page, not its point, so they sit
   * on one dense strip rather than on bordered cards. The dashboard and the
   * screens that summarise a run draw it here, so no two disagree about what a
   * count looks like.
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
