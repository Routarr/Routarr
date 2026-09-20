import { describe, it, expect, vi, afterEach } from 'vitest';
import { fireEvent, render, screen } from '@testing-library/svelte';

import ModalHarness from '../test/ModalHarness.svelte';

/**
 * A plain `<div>` overlay leaves the page behind it announced, Tab walking out,
 * Escape inert and focus unrestored. The native `<dialog>` gives all four back,
 * but only when opened with `showModal` and only when Escape is routed through
 * the caller rather than closing the element behind its back.
 */

// A hand-rolled save/restore is skipped when an assertion above it throws, and
// the next test then renders against a `showModal` that does nothing.
afterEach(() => vi.restoreAllMocks());

describe('Modal', () => {
  it('is announced as a dialog, with a name', () => {
    render(ModalHarness, { label: 'Rename category', onClose: vi.fn() });

    expect(screen.getByRole('dialog', { name: 'Rename category' })).toBeTruthy();
  });

  /**
   * `showModal` is what puts the element in the top layer and makes the rest of
   * the document inert. `open` alone renders the markup and none of that.
   */
  it('opens it as a modal, not merely as a visible element', () => {
    // jsdom ships the element and neither of its methods, so `showModal` is
    // installed here rather than spied on. That is also what makes the
    // fallback below the *default* path in this suite.
    const showModal = vi.fn();
    Object.defineProperty(HTMLDialogElement.prototype, 'showModal', {
      value: showModal,
      configurable: true,
      writable: true,
    });

    render(ModalHarness, { label: 'Rename category', onClose: vi.fn() });

    expect(showModal).toHaveBeenCalledTimes(1);

    delete (HTMLDialogElement.prototype as unknown as Record<string, unknown>).showModal;
  });

  /**
   * Escape is `cancel`, and letting the browser act on it would close the
   * element while the caller still believed the modal was showing — after which
   * nothing reopens it.
   */
  it('turns Escape into a close the caller controls', async () => {
    const onClose = vi.fn();
    render(ModalHarness, { label: 'Rename category', onClose });

    const dialog = screen.getByRole('dialog');
    const cancel = new Event('cancel', { cancelable: true, bubbles: true });
    await fireEvent(dialog, cancel);

    expect(onClose).toHaveBeenCalledTimes(1);
    expect(cancel.defaultPrevented).toBe(true);
  });

  it('takes its content away when it unmounts', () => {
    const { unmount } = render(ModalHarness, { label: 'Rename category', onClose: vi.fn() });

    expect(screen.getByText('body')).toBeTruthy();
    unmount();
    expect(screen.queryByText('body')).toBeNull();
  });

  /**
   * The fallback, which is not hypothetical: jsdom implements `<dialog>` but
   * not `showModal`, so this is the path every other test in this file takes.
   * Without it the markup would never render and none of them could assert
   * anything.
   */
  it('renders even where the test DOM has no showModal to call', () => {
    expect(
      (HTMLDialogElement.prototype as unknown as Record<string, unknown>).showModal,
    ).toBeUndefined();

    render(ModalHarness, { label: 'Rename category', onClose: vi.fn() });

    expect(screen.getByText('body')).toBeTruthy();
    expect(screen.getByRole('dialog').hasAttribute('open')).toBe(true);
  });

  /**
   * Svelte removes the block's DOM before the teardown runs, so `close()` acts
   * on a detached element and the browser's own restoration never happens: a
   * keyboard user closed a dialog and found themselves on `<body>`.
   */
  it('gives the focus back to the control that opened it', () => {
    const opener = document.createElement('button');
    opener.textContent = 'New rule';
    document.body.append(opener);
    opener.focus();

    const { unmount } = render(ModalHarness, { label: 'Rule', onClose: vi.fn() });
    // What `showModal` does in a browser: the focus moves inside.
    screen.getByRole('button', { name: 'inside' }).focus();
    expect(document.activeElement).not.toBe(opener);

    unmount();

    expect(document.activeElement).toBe(opener);
    opener.remove();
  });
});
