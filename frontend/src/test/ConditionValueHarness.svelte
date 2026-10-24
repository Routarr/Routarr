<script lang="ts">
  import { untrack } from 'svelte';

  import type { ConditionSpec, ScoreScale } from '../api/types';
  import ConditionValue from '../components/ConditionValue.svelte';

  /** Holds the value as the rule editor does, so each change comes back as the prop. */
  let {
    spec,
    initial,
    scales = [],
  }: { spec: ConditionSpec; initial: unknown; scales?: ScoreScale[] } = $props();
  let value = $state(untrack(() => initial));
  let unreadable = $state(false);
</script>

<ConditionValue
  {spec}
  {value}
  {scales}
  onChange={(next, held) => ((value = next), (unreadable = held))}
/>
<output data-testid="bound">{JSON.stringify(value)}</output>
<output data-testid="unreadable">{unreadable}</output>
