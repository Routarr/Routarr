<script lang="ts">
  import { formatPercent } from '../api/format';
  import { i18n } from '../lib/i18n.svelte';

  /**
   * How far the rules agreed, as a figure beside a meter.
   *
   * The percentage used to sit *on* the fill, centred — so its background was
   * the fill on one side of the value and the track on the other, and around
   * 50–70% the boundary ran straight under the digits. Its contrast varied
   * from 16:1 to 1.95:1 with the value, which is why it carried a text shadow.
   * Outside the bar it is always on the card, whose contrast is measured.
   *
   * A continuous ramp rather than three bands. `confidence_for` scales
   * differently per match mode — an `all` rule with one condition is 60%, an
   * `any` rule with two is 53% — so no percentage threshold can order them, and
   * the old bands painted an ordinary two-condition rule amber. A ramp claims
   * no category: it says more or less, which is all the figure measures.
   *
   * The ramp runs from *muted* to success, not from warning: starting at the
   * warning colour made an ordinary two-condition rule look like a problem,
   * which is the same claim the bands made in another form.
   *
   * The meter itself is for the explanation panel, where it has 150px to be
   * read in. A table column gives it 28px after the figure — a stub that
   * measures nothing — so there it is a dot instead: the same ramp, at a size
   * that survives the column.
   */
  let { value, meter = false }: { value: number; meter?: boolean } = $props();

  const pct = $derived(Math.round((value ?? 0) * 100));
  /// Zero is the one case worth flagging, and an empty bar cannot show it.
  const none = $derived(pct === 0);
  /**
   * Where this value sits on the ramp, as the percentage `color-mix` wants.
   *
   * Computed here rather than in `calc()` inside the mix: an unregistered
   * custom property inside a colour function leaves the whole declaration
   * invalid at computed-value time, and the fill renders transparent.
   *
   * 45% is the floor the engine can produce for a matching rule — an `any`
   * rule with one condition — so the ramp spans the range that actually
   * occurs rather than a decorative 0–100.
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
      <span class="confidence-fill" style="width: {pct}%; --mix: {mix}%"></span>
    </span>
  {/if}
</div>
