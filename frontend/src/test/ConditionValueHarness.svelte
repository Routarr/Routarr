<script lang="ts">
  import { untrack } from 'svelte';

  import type { ConditionSpec } from '../api/types';
  import ConditionValue from '../components/ConditionValue.svelte';

  /**
   * Holds the value as the rule editor does, so each change comes back as the
   * prop: the loop a one-way stub never closes.
   */
  let { spec, initial }: { spec: ConditionSpec; initial: unknown } = $props();
  let value = $state(untrack(() => initial));
</script>

<ConditionValue {spec} {value} onChange={(next) => (value = next)} />
<output data-testid="bound">{JSON.stringify(value)}</output>
