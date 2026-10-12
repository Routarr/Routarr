<script lang="ts">
  import Modal from '../components/Modal.svelte';

  /** Gives `Modal` the children snippet a test cannot pass. */
  let {
    label,
    onClose,
    closeOnBackdrop = false,
    initialFocus,
    returnFocus,
  }: {
    label: string;
    onClose: () => void;
    closeOnBackdrop?: boolean;
    initialFocus?: string;
    returnFocus?: string;
  } = $props();
</script>

<Modal {label} {onClose} {closeOnBackdrop} {initialFocus} {returnFocus}>
  <p>body</p>
  <button type="button">inside</button>
  <input id="harness-field" aria-label="field" />
</Modal>
