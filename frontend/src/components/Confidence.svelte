<script lang="ts">
  import { formatPercent } from '../api/format';
  import { i18n } from '../lib/i18n.svelte';

  /**
   * How far the rules agreed, as a figure beside a meter.
   *
   * The figure sits beside the bar, never on it: over the fill on one side of
   * its digits and the track on the other, its contrast would change with the
   * value. Beside the bar it is always on the card, whose contrast is measured.
   *
   * The meter is the guide's progress bar, for the explanation panel, where it
   * has 150px to be read in: its length is the value, and one colour claims no
   * good or bad. A table column gives it 28px after the figure, a stub that
   * measures nothing, so there it is a dot whose intensity carries the value.
   * No band or threshold: `confidence_for` scales differently per match mode,
   * an `all` rule with one condition is 60% and an `any` rule with two is 53%,
   * so no percentage can order them.
   */
  let { value, meter = false }: { value: number; meter?: boolean } = $props();

  const pct = $derived(Math.round((value ?? 0) * 100));
  /** Zero is the one case worth flagging, and an empty bar cannot show it. */
  const none = $derived(pct === 0);
  /**
   * Where this value sits on the dot's ramp, as the percentage `color-mix` wants.
   *
   * Computed here rather than in `calc()` inside the mix: an unregistered
   * custom property inside a colour function leaves the whole declaration
   * invalid at computed-value time, and the dot renders transparent.
   *
   * 45% is the floor the engine can produce for a matching rule, an `any` rule
   * with one condition, so the ramp spans the range that actually occurs rather
   * than a decorative 0 to 100.
   */
  const mix = $derived(Math.min(100, Math.max(0, ((pct - 45) / 55) * 100)));
</script>

<div class="confidence">
  <!-- The mark repeats the figure in a form a screen reader has no use for. -->
  {#if !meter}
    <i class="confidence-dot{none ? ' is-none' : ''}" style="--mix: {mix}%" aria-hidden="true"></i>
  {/if}
  <span class="confidence-value{none ? ' is-none' : ''}">{formatPercent(value, i18n.language)}</span
  >
  {#if meter}
    <span class="confidence-track" aria-hidden="true">
      <span class="confidence-fill" style="width: {pct}%"></span>
    </span>
  {/if}
</div>
