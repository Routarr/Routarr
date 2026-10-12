<script lang="ts">
  import { formatPercent } from '../api/format';
  import { i18n } from '../lib/i18n.svelte';

  /**
   * How far the rules agreed, as a figure beside a bar, or beside a dot whose intensity
   * carries the value in a table column too narrow for a bar.
   *
   * The figure never sits on the bar, where its contrast would change with the value.
   * There is no band or threshold: `confidence_for` scales differently per match mode, so no
   * percentage orders an `all` rule against an `any` one.
   */
  let { value, meter = false }: { value: number; meter?: boolean } = $props();

  const pct = $derived(Math.round((value ?? 0) * 100));
  /** Zero is the one case worth flagging, and an empty bar cannot show it. */
  const none = $derived(pct === 0);
  /**
   * Where this value sits on the dot's ramp, as the percentage `color-mix` wants.
   *
   * Computed here, not in `calc()`: an unregistered custom property inside a colour function
   * invalidates the declaration at computed-value time and the dot renders transparent.
   * The ramp starts at 45%, the lowest confidence the engine gives a matching rule.
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
