import { describe, it, expect, onTestFinished, vi } from 'vitest';
import { fireEvent, render, screen } from '@testing-library/svelte';

import ModalHarness from '../test/ModalHarness.svelte';

/**
 * A plain `<div>` overlay leaves the page behind it announced, Tab walking out,
 * Escape inert and focus unrestored. The native `<dialog>` gives all four back,
 * but only when opened with `showModal` and only when Escape is routed through
 * the caller rather than closing the element behind its back.
 */

describe('Modal', () => {
  /**
   * Beside a form, a stray click would throw away what was typed, so a click
   * outside does nothing unless the caller holds nothing to lose.
   */
  it('closes on a click beside its content only when the caller allows it', async () => {
    const guarded = vi.fn();
    const { unmount } = render(ModalHarness, { label: 'Rename category', onClose: guarded });
    await fireEvent.click(screen.getByRole('dialog'));
    expect(guarded).not.toHaveBeenCalled();
    unmount();

    const onClose = vi.fn();
    render(ModalHarness, { label: 'Quick search', onClose, closeOnBackdrop: true });
    await fireEvent.pointerDown(screen.getByText('body'));
    await fireEvent.click(screen.getByText('body'));
    expect(onClose).not.toHaveBeenCalled();
    await fireEvent.pointerDown(screen.getByRole('dialog'));
    await fireEvent.click(screen.getByRole('dialog'));
    expect(onClose).toHaveBeenCalledTimes(1);
  });

  /**
   * A click goes to the common ancestor of its press and its release. Selecting
   * the typed query by dragging past the field's edge ends in a click on the
   * dialog itself, and the query goes with a closed palette.
   */
  it('stays open when a press inside the content is released beside it', async () => {
    const onClose = vi.fn();
    render(ModalHarness, { label: 'Quick search', onClose, closeOnBackdrop: true });

    await fireEvent.pointerDown(screen.getByLabelText('field'));
    await fireEvent.click(screen.getByRole('dialog'));

    expect(onClose).not.toHaveBeenCalled();
  });

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
    // fallback below the *default* path in this suite. Taken away when the
    // test finishes, failed or not: left behind, it would stand in for the
    // fallback in every test after this one.
    const showModal = vi.fn();
    Object.defineProperty(HTMLDialogElement.prototype, 'showModal', {
      value: showModal,
      configurable: true,
      writable: true,
    });
    onTestFinished(() => {
      delete (HTMLDialogElement.prototype as unknown as Record<string, unknown>).showModal;
    });

    render(ModalHarness, { label: 'Rename category', onClose: vi.fn() });

    expect(showModal).toHaveBeenCalledTimes(1);
  });

  /**
   * Escape is `cancel`, and letting the browser act on it would close the
   * element while the caller still believes the modal is showing, after which
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
   * on a detached element and the browser's own restoration never happens, so
   * a keyboard user who closes a dialog lands on `<body>`.
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

  /**
   * The browser opens a dialog on its first focusable element, the close
   * button in a form's header, where a reflex Enter throws the form away.
   */
  it('opens on the element it is told to', () => {
    render(ModalHarness, {
      label: 'Add instance',
      onClose: vi.fn(),
      initialFocus: 'harness-field',
    });

    expect(document.activeElement).toBe(screen.getByRole('textbox', { name: 'field' }));
  });

  /**
   * A dialog a screen opens on arrival has no opener but the page. Without a
   * fallback, closing it leaves the focus on `<body>`.
   */
  it('gives the focus to its fallback when the page itself opened it', () => {
    const fallback = document.createElement('button');
    fallback.id = 'add-instance';
    document.body.append(fallback);
    (document.activeElement as HTMLElement | null)?.blur();
    expect(document.activeElement).toBe(document.body);

    const { unmount } = render(ModalHarness, {
      label: 'Add instance',
      onClose: vi.fn(),
      returnFocus: 'add-instance',
    });
    unmount();

    expect(document.activeElement).toBe(fallback);
    fallback.remove();
  });

  it('prefers the control that opened it over its fallback', () => {
    const opener = document.createElement('button');
    const fallback = document.createElement('button');
    fallback.id = 'add-instance';
    document.body.append(opener, fallback);
    opener.focus();

    const { unmount } = render(ModalHarness, {
      label: 'Edit instance',
      onClose: vi.fn(),
      returnFocus: 'add-instance',
    });
    unmount();

    expect(document.activeElement).toBe(opener);
    opener.remove();
    fallback.remove();
  });
});
