import { describe, it, expect, vi, afterEach } from 'vitest';
import { fireEvent, screen } from '@testing-library/svelte';

import { api } from '../api/client';
import { renderWithI18n } from '../test/render';
import { askPassphrase, withProof } from '../lib/proof.svelte';
import ProofDialog from './ProofDialog.svelte';

/**
 * The dialog the proof is typed into. Cancel must answer nothing: wired to an
 * empty proof, the write would go out and come back refused, as if the
 * password typed had been wrong.
 */

const STRINGS = {
  Cancel: 'Cancel',
  Continue: 'Continue',
  CurrentPassword: 'Current password',
  ProofPasswordTitle: 'Confirm with your password',
  ProofKeyTitle: 'Confirm with the API key',
  RoutarrApiKey: 'Routarr API key',
  ProofPassphraseTitle: 'Archive passphrase',
  ProofPassphraseHelp: 'This archive is encrypted.',
  Passphrase: 'Passphrase',
};

function asking(mode: 'forms' | 'apikey') {
  vi.spyOn(api, 'authMode').mockResolvedValue({
    mode,
    api_key_configured: true,
    api_key_pinned: false,
  });
  renderWithI18n(ProofDialog, { strings: STRINGS });
  const send = vi.fn().mockResolvedValue('written');
  return { send, result: withProof(send) };
}

afterEach(() => vi.restoreAllMocks());

describe('ProofDialog', () => {
  it('hands back the password typed', async () => {
    const { send, result } = asking('forms');

    expect(await screen.findByRole('dialog', { name: 'Confirm with your password' })).toBeVisible();
    const field = screen.getByLabelText('Current password');
    expect(field).toHaveAttribute('autocomplete', 'current-password');
    await fireEvent.input(field, { target: { value: 'the password' } });
    await fireEvent.click(screen.getByRole('button', { name: 'Continue' }));

    expect(await result).toBe('written');
    expect(send).toHaveBeenCalledWith({ current_password: 'the password' });
    expect(screen.queryByRole('dialog')).toBeNull();
  });

  it('asks an API key session for the key, and offers nothing to send until it is typed', async () => {
    const { send, result } = asking('apikey');

    await screen.findByRole('dialog', { name: 'Confirm with the API key' });
    const proceed = screen.getByRole('button', { name: 'Continue' });
    expect(proceed).toBeDisabled();
    await fireEvent.input(screen.getByLabelText('Routarr API key'), {
      target: { value: 'the key' },
    });
    expect(proceed).toBeEnabled();
    await fireEvent.click(proceed);

    expect(await result).toBe('written');
    expect(send).toHaveBeenCalledWith({ current_key: 'the key' });
  });

  it('answers nothing on Cancel, and forgets what was typed', async () => {
    const { send, result } = asking('forms');

    await fireEvent.input(await screen.findByLabelText('Current password'), {
      target: { value: 'half' },
    });
    await fireEvent.click(screen.getByRole('button', { name: 'Cancel' }));

    expect(await result).toBeNull();
    expect(send).not.toHaveBeenCalled();

    const again = withProof(send);
    expect(await screen.findByLabelText('Current password')).toHaveValue('');
    await fireEvent(screen.getByRole('dialog'), new Event('cancel', { cancelable: true }));
    expect(await again).toBeNull();
  });

  /** Asked again, it says why rather than repeating the first question. */
  it('asks for an archive passphrase, and says why it asks again', async () => {
    renderWithI18n(ProofDialog, { strings: STRINGS });

    const first = askPassphrase();
    expect(await screen.findByRole('dialog', { name: 'Archive passphrase' })).toBeVisible();
    expect(screen.getByText('This archive is encrypted.')).toBeVisible();
    await fireEvent.input(screen.getByLabelText('Passphrase'), { target: { value: 'wrong' } });
    await fireEvent.click(screen.getByRole('button', { name: 'Continue' }));
    expect(await first).toBe('wrong');

    const again = askPassphrase('That passphrase does not open this archive.');
    expect(await screen.findByRole('alert')).toHaveTextContent('does not open this archive');
    expect(screen.queryByText('This archive is encrypted.')).toBeNull();
    await fireEvent(screen.getByRole('dialog'), new Event('cancel', { cancelable: true }));
    expect(await again).toBeNull();
  });
});
