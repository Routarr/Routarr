<script lang="ts">
  import Modal from '../components/Modal.svelte';

  /**
   * A component cannot be handed a snippet from a test, so the snippet lives
   * here. This is the smallest thing that can render `Modal` with children.
   */
  let {
    label,
    onClose,
    closeOnBackdrop = false,
  }: { label: string; onClose: () => void; closeOnBackdrop?: boolean } = $props();
</script>

<Modal {label} {onClose} {closeOnBackdrop}>
  <p>body</p>
  <button type="button">inside</button>
</Modal>
