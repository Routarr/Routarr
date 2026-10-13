<script lang="ts">
  import Pager from '../components/Pager.svelte';
  import type { Paginated } from '../api/types';

  /** Binds `page` where a test can read it. */
  let { pagination }: { pagination: Paginated<unknown>['pagination'] | undefined } = $props();
  let page = $state(1);
</script>

<Pager {pagination} bind:page countKey="ItemCount" />
<output data-testid="page">{page}</output>
