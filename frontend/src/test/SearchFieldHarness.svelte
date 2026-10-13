<script lang="ts">
  import SearchField from '../components/SearchField.svelte';

  /** Binds `value` where a test can read what the parent sees, and when. */
  let { debounce = 0 }: { debounce?: number } = $props();
  let value = $state('');
</script>

<SearchField bind:value placeholder="Search" label="Search" {debounce} />
<output data-testid="bound">{value}</output>
