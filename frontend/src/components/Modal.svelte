<script lang="ts">
  import { untrack, type Snippet } from 'svelte';

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
    closeOnBackdrop = false,
    initialFocus,
    returnFocus,
    children,
  }: {
    /** Names the dialog for assistive technology; usually the visible title. */
    label: string;
    onClose: () => void;
    /** Matches the width each screen already used for its own content box. */
    maxWidth?: number;
    maxHeight?: string;
    /**
     * Whether a click beside the content closes it. Off by default: beside a
     * form, a stray click would throw away what was typed. For a dialog that
     * holds nothing to lose, like the quick search.
     */
    closeOnBackdrop?: boolean;
    /**
     * The id of the element that takes the focus on opening, a form's first
     * field. Left to the browser, the focus lands on the first focusable
     * element, the close button in a form's header, where a reflex Enter
     * throws the form away.
     */
    initialFocus?: string;
    /**
     * The id of the control that takes the focus back when no control opened
     * the dialog, as when a screen opens it on arrival from the guide, or
     * when the one that did is gone. The focus would otherwise fall to the
     * page, where a screen reader loses its place.
     */
    returnFocus?: string;
    children: Snippet;
  } = $props();

  let dialog = $state<HTMLDialogElement | null>(null);
  let pressedOnBackdrop = false;

  $effect(() => {
    const element = dialog;
    if (!element) return;

    // Read before `showModal` moves it: this is the control that opened the
    // dialog, and where the focus has to go back to. `<body>` opened nothing.
    const active = document.activeElement;
    const opener = active instanceof HTMLElement && active !== document.body ? active : null;
    // Read once: a change re-running this effect would close the dialog and
    // open it again.
    const [first, fallback] = untrack(() => [initialFocus, returnFocus]);

    // `showModal` is what puts the element in the top layer and makes the rest
    // of the document inert; the fallback is for test DOMs that stop short of
    // implementing it, where the markup still has to render.
    if (typeof element.showModal === 'function') {
      if (!element.open) element.showModal();
    } else {
      element.setAttribute('open', '');
    }
    if (first) document.getElementById(first)?.focus();

    return () => {
      if (element.open && typeof element.close === 'function') element.close();
      // The block's DOM is removed before this runs, so `close()` acts on a
      // detached element and the browser's own restoration never happens: a
      // keyboard user landed on `<body>` and started over from the top of the
      // page. Done by hand, for every way of closing — Escape, Cancel, a save.
      const back = opener?.isConnected ? opener : fallback && document.getElementById(fallback);
      if (back && !element.contains(back)) back.focus();
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

<!-- The click is the pointer's way out and Escape the keyboard's, through
     `oncancel` below, so no key handler belongs on this element. -->
<dialog
  bind:this={dialog}
  class="modal-overlay"
  aria-label={label}
  oncancel={(event) => {
    event.preventDefault();
    onClose();
  }}
  onpointerdown={(event) => {
    pressedOnBackdrop = event.target === event.currentTarget;
  }}
  onclick={(event) => {
    // A click goes to the common ancestor of its press and its release, so a
    // drag from the field released beside it lands on the dialog itself.
    if (closeOnBackdrop && pressedOnBackdrop && event.target === event.currentTarget) onClose();
  }}
>
  <div class="modal-content" {style}>
    {@render children()}
  </div>
</dialog>
