<script lang="ts">
  import { keepInAddress } from '../lib/address.svelte';

  /** `keepInAddress` opens effects, so it runs inside a component, as a list screen calls it. */
  let outcome = $state('');
  let search = $state('');
  let read = $state(0);

  keepInAddress(
    () => ({ outcome, search }),
    (from) => {
      outcome = from('outcome', ['success', 'failure']);
      search = from('search');
      read += 1;
    },
  );
</script>

<input aria-label="Search" bind:value={search} />
<select aria-label="Outcome" bind:value={outcome}>
  <option value="">All</option>
  <option value="success">Success</option>
  <option value="failure">Failure</option>
</select>
<p>Read {read}</p>
