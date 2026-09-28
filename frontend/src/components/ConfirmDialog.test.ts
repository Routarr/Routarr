import { describe, it, expect } from 'vitest';
import { fireEvent, screen } from '@testing-library/svelte';

import { renderWithI18n } from '../test/render';
import { ask, askConfirmation } from '../lib/confirm.svelte';
import ConfirmDialog from './ConfirmDialog.svelte';

/**
 * The question in front of every deletion, move and replace.
 *
 * Cancel is the answer that matters: wired to anything but `null`, every
 * guarded action would go ahead on the button meant to stop it, and every page
 * test answers through the store, never through these buttons.
 */

const STRINGS = {
  Cancel: 'Cancel',
  Delete: 'Delete',
  ImportAppend: 'Add',
  ImportReplace: 'Replace',
};

const show = () => renderWithI18n(ConfirmDialog, { strings: STRINGS });

describe('ConfirmDialog', () => {
  it('answers nothing when Cancel is pressed', async () => {
    show();
    const answer = askConfirmation('Delete "Anime"?', 'Delete');

    await fireEvent.click(await screen.findByRole('button', { name: 'Cancel' }));

    expect(await answer).toBe(false);
    expect(screen.queryByRole('dialog')).toBeNull();
  });

  it('answers nothing on Escape', async () => {
    show();
    const answer = ask('Replace the rules, or add to them?', [
      { label: 'ImportAppend', value: 'append' },
      { label: 'ImportReplace', value: 'replace', danger: true },
    ]);

    // Escape reaches a `<dialog>` as its `cancel` event.
    await fireEvent(await screen.findByRole('dialog'), new Event('cancel', { cancelable: true }));

    expect(await answer).toBeNull();
  });

  it('answers the value of the choice pressed', async () => {
    show();
    const answer = ask('Replace the rules, or add to them?', [
      { label: 'ImportAppend', value: 'append' },
      { label: 'ImportReplace', value: 'replace', danger: true },
    ]);

    await fireEvent.click(await screen.findByRole('button', { name: 'Replace' }));

    expect(await answer).toBe('replace');
  });

  /** A question left waiting would hold its caller for good. */
  it('releases a first question with no answer when a second one is asked', async () => {
    show();
    const first = askConfirmation('Delete "Anime"?', 'Delete');
    const second = askConfirmation('Delete "Kids"?', 'Delete');

    expect(await first).toBe(false);
    expect(await screen.findByText('Delete "Kids"?')).toBeInTheDocument();
    await fireEvent.click(screen.getByRole('button', { name: 'Delete' }));
    expect(await second).toBe(true);
  });
});
