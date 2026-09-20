<script lang="ts">
  import type { Snippet } from 'svelte';

  /**
   * A modal dialog, built on the native `<dialog>` element.
   *
   * A hand-rolled overlay is a plain `<div>`: a screen reader keeps announcing
   * the page behind it, Tab walks straight out of it, Escape does nothing, and
   * closing it leaves the focus wherever it happened to be. `<dialog>` opened
   * with `showModal()` gives all four from the browser — the top layer, the
   * focus trap, the inert background and focus restored to whatever opened it —
   * instead of four hand-written approximations that drift apart.
   *
   * `oncancel` is the Escape key. It is prevented and turned into `onClose` so the
   * caller's own state stays the single source of truth for whether it is open —
   * letting the browser close the element directly would leave the caller
   * convinced it was still showing.
   *
   * The one thing the browser does *not* give back is the focus: see the
   * effect's teardown.
   */
  let {
    label,
    onClose,
    maxWidth,
    maxHeight,
    children,
  }: {
    /** Names the dialog for assistive technology; usually the visible title. */
    label: string;
    onClose: () => void;
    /** Matches the width each screen already used for its own content box. */
    maxWidth?: number;
    maxHeight?: string;
    children: Snippet;
  } = $props();

  let dialog = $state<HTMLDialogElement | null>(null);

  $effect(() => {
    const element = dialog;
    if (!element) return;

    // Read before `showModal` moves it: this is the control that opened the
    // dialog, and where the focus has to go back to.
    const opener = document.activeElement instanceof HTMLElement ? document.activeElement : null;

    // `showModal` is what puts the element in the top layer and makes the rest
    // of the document inert; the fallback is for test DOMs that stop short of
    // implementing it, where the markup still has to render.
    if (typeof element.showModal === 'function') {
      if (!element.open) element.showModal();
    } else {
      element.setAttribute('open', '');
    }

    return () => {
      if (element.open && typeof element.close === 'function') element.close();
      // The block's DOM is removed before this runs, so `close()` acts on a
      // detached element and the browser's own restoration never happens: a
      // keyboard user landed on `<body>` and started over from the top of the
      // page. Done by hand, for every way of closing — Escape, Cancel, a save.
      if (opener?.isConnected && !element.contains(opener)) opener.focus();
    };
  });

  const style = $derived(
    [
      maxWidth === undefined ? '' : `max-width: ${maxWidth}px`,
      maxHeight === undefined ? '' : `max-height: ${maxHeight}`,
      'overflow-y: auto',
    ]
      .filter(Boolean)
      .join('; '),
  );
</script>

<dialog
  bind:this={dialog}
  class="modal-overlay"
  aria-label={label}
  oncancel={(event) => {
    event.preventDefault();
    onClose();
  }}
>
  <div class="modal-content" {style}>
    {@render children()}
  </div>
</dialog>
