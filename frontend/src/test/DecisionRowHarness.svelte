<script lang="ts">
  import DecisionRow from '../components/DecisionRow.svelte';
  import type { Decision } from '../api/types';

  /**
   * `DecisionRow` renders a `<tr>`, which the HTML parser drops unless it is
   * inside a table. This puts it in one.
   */
  let {
    decision,
    selected,
    onToggle,
  }: { decision: Decision; selected: boolean; onToggle: () => void } = $props();
</script>

<table>
  <tbody>
    <DecisionRow {decision} {selected} {onToggle} />
  </tbody>
</table>
