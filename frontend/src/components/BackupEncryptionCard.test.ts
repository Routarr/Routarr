import { describe, it, expect, vi, afterEach } from 'vitest';
import { fireEvent, screen, waitFor } from '@testing-library/svelte';

import { renderWithI18n } from '../test/render';
import { answerConfirmation } from '../test/confirm';
import { ApiError, api } from '../api/client';
import { createOutcome } from '../lib/outcome.svelte';
import BackupEncryptionCard from './BackupEncryptionCard.svelte';

/**
 * The passphrase every archive is sealed with. What can go wrong unseen: a
 * passphrase typed twice differently, which seals every archive with words
 * nobody knows, and encryption stopped by a click nobody confirmed.
 */

// The proof has its own tests (`lib/proof.test.ts`): here it is given, as the
// dialog would hand it back.
vi.mock('../lib/proof.svelte', () => ({
  withProof: <T>(send: (proof: object) => Promise<T>) => send({ current_key: 'the-key-proven' }),
}));

const STRINGS = {
  BackupEncryption: 'Backup encryption',
  BackupEncryptionOn: 'Archives are encrypted.',
  BackupEncryptionOff: 'Archives are not encrypted.',
  NewPassphrase: 'New passphrase',
  RepeatPassphrase: 'Repeat the passphrase',
  PassphrasesDiffer: 'The two passphrases differ.',
  PasswordMinimum: 'At least {min} characters.',
  EncryptArchives: 'Encrypt the archives',
  ChangePassphrase: 'Change the passphrase',
  StopEncrypting: 'Stop encrypting',
  ConfirmStopEncrypting: 'Stop encrypting the archives?',
  PassphraseSaved: 'Passphrase saved.',
  EncryptionStopped: 'Encryption stopped.',
};

const LONG_ENOUGH = 'twelve chars';

function mount(configured: boolean) {
  const outcome = createOutcome();
  renderWithI18n(BackupEncryptionCard, { props: { outcome, configured }, strings: STRINGS });
  return outcome;
}

async function type(label: string, value: string) {
  await fireEvent.input(screen.getByLabelText(label), { target: { value } });
}

afterEach(() => vi.restoreAllMocks());

describe('BackupEncryptionCard', () => {
  it('encrypts the archives with a passphrase typed twice alike, with the proof sent', async () => {
    const set = vi.spyOn(api, 'setBackupPassphrase').mockResolvedValue(undefined);
    const outcome = mount(false);
    expect(screen.getByText('Archives are not encrypted.')).toBeInTheDocument();
    const encrypt = screen.getByRole('button', { name: 'Encrypt the archives' });

    await type('New passphrase', 'eleven char');
    await type('Repeat the passphrase', 'eleven char');
    expect(screen.getByLabelText('New passphrase')).toHaveAttribute('aria-invalid', 'true');
    expect(encrypt).toBeDisabled();

    await type('New passphrase', LONG_ENOUGH);
    await type('Repeat the passphrase', 'twelve chars!');
    expect(screen.getByText('The two passphrases differ.')).toBeInTheDocument();
    expect(encrypt).toBeDisabled();

    await type('Repeat the passphrase', LONG_ENOUGH);
    await fireEvent.click(encrypt);

    expect(set).toHaveBeenCalledWith(LONG_ENOUGH, { current_key: 'the-key-proven' });
    await waitFor(() => expect(outcome.notice).toBe('Passphrase saved.'));
    expect(screen.getByText('Archives are encrypted.')).toBeInTheDocument();
    expect(screen.getByLabelText('New passphrase')).toHaveValue('');
    expect(screen.getByRole('button', { name: 'Change the passphrase' })).toBeInTheDocument();
  });

  it('stops encrypting only once confirmed, with the proof sent', async () => {
    const set = vi.spyOn(api, 'setBackupPassphrase').mockResolvedValue(undefined);
    const outcome = mount(true);

    await fireEvent.click(screen.getByRole('button', { name: 'Stop encrypting' }));
    await answerConfirmation(null);
    expect(set).not.toHaveBeenCalled();

    await fireEvent.click(screen.getByRole('button', { name: 'Stop encrypting' }));
    expect(await answerConfirmation()).toBe('Stop encrypting the archives?');

    expect(set).toHaveBeenCalledWith('', { current_key: 'the-key-proven' });
    await waitFor(() => expect(outcome.notice).toBe('Encryption stopped.'));
    expect(screen.getByText('Archives are not encrypted.')).toBeInTheDocument();
    expect(screen.queryByRole('button', { name: 'Stop encrypting' })).toBeNull();
  });

  it('says a refusal and keeps what was typed', async () => {
    vi.spyOn(api, 'setBackupPassphrase').mockRejectedValue(
      new ApiError('That is not the current API key.', 403, 'forbidden'),
    );
    const outcome = mount(false);

    await type('New passphrase', LONG_ENOUGH);
    await type('Repeat the passphrase', LONG_ENOUGH);
    await fireEvent.click(screen.getByRole('button', { name: 'Encrypt the archives' }));

    await waitFor(() => expect(outcome.error).toBe('That is not the current API key.'));
    expect(screen.getByLabelText('New passphrase')).toHaveValue(LONG_ENOUGH);
    expect(screen.getByText('Archives are not encrypted.')).toBeInTheDocument();
  });
});
