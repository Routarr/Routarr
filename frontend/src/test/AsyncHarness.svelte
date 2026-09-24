<script lang="ts">
  import { createAsync } from '../lib/async.svelte';

  /**
   * `createAsync` opens an effect, so it has to run inside a component; this
   * is the smallest one that shows what a page would read from it. `filter` is
   * the dependency a screen would pass as `deps`: a test re-renders with a new
   * one to move the inputs on.
   */
  let { loader, filter }: { loader: (signal: AbortSignal) => Promise<string>; filter: string } =
    $props();
  const value = createAsync(
    (signal) => loader(signal),
    () => filter,
  );

  /** What an action handler calls after its write, exposed so a test can call it late. */
  export function reload() {
    return value.reload();
  }
</script>

<output data-testid="data">{value.data ?? ''}</output>
<output data-testid="loading">{value.loading ? 'loading' : 'idle'}</output>
<output data-testid="error">{value.error ?? ''}</output>
