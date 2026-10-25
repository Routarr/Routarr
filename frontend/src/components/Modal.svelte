<script lang="ts">
  import { untrack, type Snippet } from 'svelte';
  import { X } from '../lib/icons';
  import { t } from '../lib/i18n.svelte';

  /**
   * A modal dialog on the native `<dialog>`, opened with `showModal()` for the top layer, the
   * focus trap and the inert page. Escape (`oncancel`) is prevented and turned into `onClose`,
   * so the caller's state alone says whether it is open. The focus goes back by hand: see the
   * effect's teardown.
   */
  let {
    label,
    title,
    closable = true,
    titleId,
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
    /** Names a dialog that shows no title, or one whose name says more than its title. */
    label?: string;
    /** The header's heading, which names the dialog unless `label` does. */
    title?: string;
    /** Whether the header offers a close button: off where the dialog's Cancel is the way out. */
    closable?: boolean;
    /** The heading's id, which makes it focusable, for a dialog that opens on its heading. */
    titleId?: string;
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
  const uid = $props.id();
  const heading = $derived(titleId ?? `${uid}-title`);

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
  aria-labelledby={label || !title ? undefined : heading}
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
    {#if title}
      <div class="modal-header">
        <!-- Focusable only where asked: a browser opens a dialog on a focusable heading. -->
        {#if titleId}
          <h2 id={heading} class="modal-title" tabindex="-1">{title}</h2>
        {:else}
          <h2 id={heading} class="modal-title">{title}</h2>
        {/if}
        {#if closable}
          <button
            type="button"
            class="close-button"
            onclick={onClose}
            aria-label={t('Dismiss')}
            title={t('Dismiss')}
          >
            <X size={18} aria-hidden="true" />
          </button>
        {/if}
      </div>
    {/if}
    {@render children()}
  </div>
</dialog>
