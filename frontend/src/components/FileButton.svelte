<script lang="ts">
  import { Upload } from '../lib/icons';
  import FilePicker from './FilePicker.svelte';

  /**
   * A button that opens the file picker and hands over the file chosen. A real button, since a
   * label styled as one takes no focus.
   */
  let { label, accept, onFile }: { label: string; accept: string; onFile: (file: File) => void } =
    $props();

  let picker = $state<ReturnType<typeof FilePicker> | null>(null);
</script>

<button type="button" class="btn btn-secondary" onclick={() => picker?.pick()}>
  <Upload size={16} />
  {label}
</button>
<FilePicker bind:this={picker} {label} {accept} {onFile} />
