import { describe, it, expect, onTestFinished, vi } from 'vitest';
import { fireEvent, render, screen } from '@testing-library/svelte';

import ModalHarness from '../test/ModalHarness.svelte';

describe('Modal', () => {
  /** Beside a form, a stray click would throw away what was typed. */
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
   * A click lands on the common ancestor of press and release, so selecting the query by
   * dragging past the field's edge clicks the dialog itself.
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

  /** Only `showModal` puts the dialog in the top layer and makes the page behind it inert. */
  it('opens it as a modal, not merely as a visible element', () => {
    // jsdom has no `showModal` to spy on. Removed pass or fail: every other test here needs
    // the fallback.
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

  /** Left to the browser, Escape closes the element while the caller still thinks it open. */
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

  /** jsdom has no `showModal`, so every other test in this file runs this path. */
  it('renders even where the test DOM has no showModal to call', () => {
    expect(
      (HTMLDialogElement.prototype as unknown as Record<string, unknown>).showModal,
    ).toBeUndefined();

    render(ModalHarness, { label: 'Rename category', onClose: vi.fn() });

    expect(screen.getByText('body')).toBeTruthy();
    expect(screen.getByRole('dialog').hasAttribute('open')).toBe(true);
  });

  /**
   * Svelte removes the DOM before the teardown runs, so `close()` acts on a detached element
   * and the browser never restores the focus itself.
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
   * The browser focuses the first focusable element, a form's close button, where a reflex
   * Enter throws the form away.
   */
  it('opens on the element it is told to', () => {
    render(ModalHarness, {
      label: 'Add instance',
      onClose: vi.fn(),
      initialFocus: 'harness-field',
    });

    expect(document.activeElement).toBe(screen.getByRole('textbox', { name: 'field' }));
  });

  /** A dialog a screen opens on arrival has no opener to hand the focus back to. */
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
