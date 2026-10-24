import { describe, it, expect, vi, afterEach } from 'vitest';
import { fireEvent, screen, waitFor } from '@testing-library/svelte';

import { renderWithI18n } from '../test/render';
import { backendConstant } from '../test/backend';
import { answerConfirmation } from '../test/confirm';
import { ApiError, api } from '../api/client';
import { createOutcome } from '../lib/outcome.svelte';
import BackupEncryptionCard from './BackupEncryptionCard.svelte';

// The proof has its own tests (`lib/proof.test.ts`): here it is given, as the
// dialog would hand it back.
vi.mock('../lib/proof.svelte', () => ({
  withProof: <T>(send: (proof: object) => Promise<T>) => send({ current_key: 'the-key-proven' }),
}));

/** The server's minimum, which the card holds a passphrase to before sending it. */
const SHORTEST = backendConstant('backend/src/services/backup/sealed.rs', 'MIN_LENGTH');
const LONG_ENOUGH = 'x'.repeat(SHORTEST);

function mount(configured: boolean) {
  const outcome = createOutcome();
  renderWithI18n(BackupEncryptionCard, { props: { outcome, configured } });
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
    expect(screen.getByText('Not encrypted')).toBeInTheDocument();
    const encrypt = screen.getByRole('button', { name: 'Encrypt the archives' });
    expect(screen.getByLabelText('New passphrase')).toHaveAccessibleDescription(
      new RegExp(`^At least ${SHORTEST} characters\\. `),
    );

    await type('New passphrase', LONG_ENOUGH.slice(1));
    await type('Repeat the passphrase', LONG_ENOUGH.slice(1));
    expect(screen.getByLabelText('New passphrase')).toHaveAttribute('aria-invalid', 'true');
    expect(encrypt).toBeDisabled();

    await type('New passphrase', LONG_ENOUGH);
    await type('Repeat the passphrase', `${LONG_ENOUGH}!`);
    expect(screen.getByText('The two passphrases differ.')).toBeInTheDocument();
    expect(encrypt).toBeDisabled();

    await type('Repeat the passphrase', LONG_ENOUGH);
    encrypt.focus();
    await fireEvent.click(encrypt);

    expect(set).toHaveBeenCalledWith(LONG_ENOUGH, { current_key: 'the-key-proven' });
    await waitFor(() =>
      expect(outcome.notice).toBe(
        'Passphrase saved. The archives on disk are being converted, see Tasks.',
      ),
    );
    expect(screen.getByText('Encrypted')).toBeInTheDocument();
    expect(screen.getByLabelText('New passphrase')).toHaveValue('');
    expect(screen.getByRole('button', { name: 'Change the passphrase' })).toBeInTheDocument();
    // The button stays disabled with the fields emptied, so the first field takes the focus.
    await waitFor(() =>
      expect(document.activeElement).toBe(screen.getByLabelText('New passphrase')),
    );
  });

  it('stops encrypting only once confirmed, with the proof sent', async () => {
    const set = vi.spyOn(api, 'setBackupPassphrase').mockResolvedValue(undefined);
    const outcome = mount(true);

    await fireEvent.click(screen.getByRole('button', { name: 'Stop encrypting' }));
    await answerConfirmation(null);
    expect(set).not.toHaveBeenCalled();

    screen.getByRole('button', { name: 'Stop encrypting' }).focus();
    await fireEvent.click(screen.getByRole('button', { name: 'Stop encrypting' }));
    expect(await answerConfirmation()).toBe(
      'Stop encrypting the archives? Every archive on disk is opened and kept in the clear, master key included.',
    );

    expect(set).toHaveBeenCalledWith('', { current_key: 'the-key-proven' });
    await waitFor(() =>
      expect(outcome.notice).toBe(
        'Encryption stopped. The archives on disk are being opened, see Tasks.',
      ),
    );
    expect(screen.getByText('Not encrypted')).toBeInTheDocument();
    expect(screen.queryByRole('button', { name: 'Stop encrypting' })).toBeNull();
    await waitFor(() =>
      expect(document.activeElement).toBe(screen.getByLabelText('New passphrase')),
    );
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
    expect(screen.getByText('Not encrypted')).toBeInTheDocument();
  });
});
