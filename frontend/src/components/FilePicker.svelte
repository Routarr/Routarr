<script lang="ts">
  /**
   * The browser's file picker, opened by `pick()` from a control of the caller's, a button or a
   * menu item. The input is emptied after each choice, so picking the same file again still
   * fires `change`.
   */
  let { label, accept, onFile }: { label: string; accept: string; onFile: (file: File) => void } =
    $props();

  let input = $state<HTMLInputElement | null>(null);

  export function pick() {
    input?.click();
  }
</script>

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
