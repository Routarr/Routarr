<script lang="ts">
  import SearchField from '../components/SearchField.svelte';

  /** Binds `value` where a test can read it: the point of the field is what the parent sees, and when. */
  let { debounce = 0 }: { debounce?: number } = $props();
  let value = $state('');
</script>

<SearchField bind:value placeholder="Search" label="Search" {debounce} />
<output data-testid="bound">{value}</output>
