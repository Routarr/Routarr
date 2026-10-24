<script lang="ts">
  import { untrack, type Snippet } from 'svelte';

  /**
   * A modal dialog on the native `<dialog>`, opened with `showModal()` for the top layer, the
   * focus trap and the inert page. Escape (`oncancel`) is prevented and turned into `onClose`,
   * so the caller's state alone says whether it is open. The focus goes back by hand: see the
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
    alert = false,
    describedBy,
    children,
  }: {
    /** Names the dialog for assistive technology, usually the visible title. */
    label: string;
    onClose: () => void;
    /** The width of the content box, chosen by each screen for its content. */
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
    /**
     * A question that interrupts, which a screen reader announces as one: an
     * `alertdialog`, named by `label` and described by `describedBy`.
     */
    alert?: boolean;
    /** The id of the text that describes the dialog beyond its name. */
    describedBy?: string;
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
    // of the document inert. The fallback is for test DOMs that stop short of
    // implementing it, where the markup still has to render.
    if (typeof element.showModal === 'function') {
      if (!element.open) element.showModal();
    } else {
      element.setAttribute('open', '');
    }
    if (first) document.getElementById(first)?.focus();

    return () => {
      if (element.open && typeof element.close === 'function') element.close();
      // The block's DOM is gone before this runs, so `close()` acts on a detached element and
      // the browser restores no focus. Done by hand, for every way of closing.
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
  role={alert ? 'alertdialog' : undefined}
  aria-label={label}
  aria-describedby={describedBy}
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
