<script lang="ts">
  import { Upload } from '../lib/icons';

  /**
   * A button that opens the file picker and hands over the file chosen.
   *
   * A label dressed as a button over a hidden input takes no focus, so no
   * keyboard reaches it. The button does, and opens the input, which stays out
   * of sight and out of the tab order. The input is emptied after each choice,
   * so choosing the same file again still counts.
   */
  let { label, accept, onFile }: { label: string; accept: string; onFile: (file: File) => void } =
    $props();

  let input = $state<HTMLInputElement | null>(null);
</script>

<button type="button" class="btn btn-secondary" onclick={() => input?.click()}>
  <Upload size={16} />
  {label}
</button>
<input
  bind:this={input}
  type="file"
  {accept}
  class="visually-hidden"
  tabindex="-1"
  aria-hidden="true"
  aria-label={label}
  onchange={(event) => {
    const file = event.currentTarget.files?.[0];
    if (file) onFile(file);
    event.currentTarget.value = '';
  }}
/>
