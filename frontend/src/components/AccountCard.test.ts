import { describe, it, expect, vi, afterEach } from 'vitest';
import { fireEvent, screen, waitFor } from '@testing-library/svelte';

import { renderWithI18n } from '../test/render';
import { backendConstant } from '../test/backend';
import { ApiError, api } from '../api/client';
import { createOutcome } from '../lib/outcome.svelte';
import AccountCard from './AccountCard.svelte';

/**
 * The password of the `forms` account. What can go wrong unseen: a new
 * password typed twice differently, which locks the owner out at the next
 * sign-in, and a key minted by the revocation that nobody gets to copy.
 */

/** The server's minimum, which the card holds a new password to before sending it. */
const SHORTEST = backendConstant('backend/src/api/auth.rs', 'MIN_PASSWORD_LENGTH');
const LONG_ENOUGH = 'x'.repeat(SHORTEST);

function mount() {
  const outcome = createOutcome();
  renderWithI18n(AccountCard, { props: { outcome } });
  return outcome;
}

async function type(label: string, value: string) {
  await fireEvent.input(screen.getByLabelText(label), { target: { value } });
}

async function fill(current: string, next: string, repeated = next) {
  await type('Current password', current);
  await type('New password', next);
  await type('Repeat the new password', repeated);
}

afterEach(() => vi.restoreAllMocks());

describe('AccountCard', () => {
  it('changes the password against the current one, then clears every field', async () => {
    const change = vi.spyOn(api, 'changePassword').mockResolvedValue({ ok: true, api_key: null });
    const outcome = mount();

    await fill('the old one', LONG_ENOUGH);
    const send = screen.getByRole('button', { name: 'Change password' });
    send.focus();
    await fireEvent.click(send);

    expect(change).toHaveBeenCalledWith('the old one', LONG_ENOUGH, false);
    await waitFor(() =>
      expect(outcome.notice).toBe('Password changed. Every other session was ended.'),
    );
    for (const label of ['Current password', 'New password', 'Repeat the new password']) {
      expect(screen.getByLabelText(label)).toHaveValue('');
    }
    // The button stays disabled with the fields emptied, so the first field takes the focus.
    await waitFor(() =>
      expect(document.activeElement).toBe(screen.getByLabelText('Current password')),
    );
  });

  it('offers nothing to send until the new password is long enough and typed twice alike', async () => {
    mount();
    const send = screen.getByRole('button', { name: 'Change password' });
    expect(screen.getByText(`At least ${SHORTEST} characters.`)).toBeInTheDocument();

    await fill('the old one', LONG_ENOUGH.slice(1));
    expect(screen.getByLabelText('New password')).toHaveAttribute('aria-invalid', 'true');
    expect(send).toBeDisabled();

    await fill('the old one', LONG_ENOUGH, `${LONG_ENOUGH}!`);
    expect(screen.getByText('The two passwords differ.')).toBeInTheDocument();
    expect(send).toBeDisabled();

    await type('Repeat the new password', LONG_ENOUGH);
    expect(screen.queryByText('The two passwords differ.')).toBeNull();
    expect(send).toBeEnabled();
  });

  it('shows the API key a revocation replaced the old one with, once', async () => {
    const change = vi
      .spyOn(api, 'changePassword')
      .mockResolvedValue({ ok: true, api_key: 'the-new-key' });
    const outcome = mount();

    await fill('the old one', LONG_ENOUGH);
    await fireEvent.click(screen.getByRole('checkbox', { name: /Also revoke every key/ }));
    await fireEvent.click(screen.getByRole('button', { name: 'Change password' }));

    expect(change).toHaveBeenCalledWith('the old one', LONG_ENOUGH, true);
    expect(await screen.findByText('the-new-key')).toBeInTheDocument();
    expect(
      screen.getByText('Copy it now. It is shown once and cannot be read again.'),
    ).toBeInTheDocument();
    expect(screen.getByRole('button', { name: 'Copy – API key' })).toBeInTheDocument();
    expect(outcome.notice).toBe(
      'Password changed. Every other session was ended and every key revoked.',
    );
    // Left ticked, the next change would revoke the keys just handed out.
    expect(screen.getByRole('checkbox', { name: /Also revoke every key/ })).not.toBeChecked();
  });

  it('says a wrong current password and keeps what was typed', async () => {
    vi.spyOn(api, 'changePassword').mockRejectedValue(
      new ApiError('The current password is wrong.', 403, 'forbidden'),
    );
    const outcome = mount();

    await fill('not it', LONG_ENOUGH);
    await fireEvent.click(screen.getByRole('button', { name: 'Change password' }));

    await waitFor(() => expect(outcome.error).toBe('The current password is wrong.'));
    expect(screen.getByLabelText('New password')).toHaveValue(LONG_ENOUGH);
    expect(screen.getByRole('button', { name: 'Change password' })).toBeEnabled();
  });
});
