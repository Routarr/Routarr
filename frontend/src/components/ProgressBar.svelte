<script lang="ts">
  import { formatCount } from '../api/format';
  import { i18n } from '../lib/i18n.svelte';

  /**
   * How far a task has gone, as a bar and its two figures. The Tasks screen
   * draws one per task, and a screen following its own task draws that one,
   * so the two never disagree about what progress looks like.
   */
  let { current, total, label }: { current: number; total: number; label: string } = $props();

  const done = $derived(Math.min(current, total));
</script>

<div class="flex items-center gap-2">
  <div
    class="progress-track"
    role="progressbar"
    aria-label={label}
    aria-valuemin={0}
    aria-valuemax={total}
    aria-valuenow={done}
  >
    <div class="progress-fill" style="width: {total > 0 ? (done / total) * 100 : 0}%"></div>
  </div>
  <span class="mono text-sm">
    {formatCount(current, i18n.language)}/{formatCount(total, i18n.language)}
  </span>
</div>
