import { describe, it, expect, vi, afterEach } from 'vitest';
import { screen, waitFor } from '@testing-library/svelte';
import userEvent from '@testing-library/user-event';

import { renderWithI18n } from '../test/render';
import { answerConfirmation } from '../test/confirm';
import { api } from '../api/client';
import { createOutcome } from '../lib/outcome.svelte';
import WebhookSigning from './WebhookSigning.svelte';

/**
 * The secret is shown once, as it is made, and replacing or stopping it is
 * asked about first: a receiver checking signatures refuses what follows.
 */

function show(signed: boolean, readable = true) {
  vi.spyOn(api, 'webhookSigning').mockResolvedValue({
    signed,
    since: signed ? '2026-09-30 12:00:00' : null,
    readable,
  });
  return renderWithI18n(WebhookSigning, {
    props: { outcome: createOutcome() },
  });
}

afterEach(() => vi.restoreAllMocks());

describe('WebhookSigning', () => {
  it('generates a secret without asking and shows it once', async () => {
    const user = userEvent.setup();
    show(false);
    const rotate = vi.spyOn(api, 'rotateWebhookSigning').mockResolvedValue({ secret: 'whsec_abc' });

    expect(await screen.findByText('Notifications are not signed.')).toBeTruthy();
    expect(screen.queryByRole('button', { name: 'Stop signing' })).toBeNull();
    await user.click(screen.getByRole('button', { name: 'Generate a signing secret' }));

    await waitFor(() => expect(rotate).toHaveBeenCalled());
    expect(await screen.findByText('whsec_abc')).toBeTruthy();
    expect(
      screen.getByText('Copy it now. It is shown once and cannot be read again.'),
    ).toBeTruthy();
    expect(screen.getByRole('button', { name: 'Copy – Signing secret' })).toBeTruthy();
  });

  it('asks before replacing a secret a receiver may be checking', async () => {
    const user = userEvent.setup();
    show(true);
    const rotate = vi.spyOn(api, 'rotateWebhookSigning').mockResolvedValue({ secret: 'whsec_new' });

    await user.click(await screen.findByRole('button', { name: 'Replace the secret' }));
    expect(await answerConfirmation(null)).toBe(
      'Replace the signing secret? The current one keeps signing beside the new one for a day, then stops.',
    );
    expect(rotate).not.toHaveBeenCalled();
  });

  it('replaces the secret once asked, and shows the new one', async () => {
    const user = userEvent.setup();
    show(true);
    const rotate = vi.spyOn(api, 'rotateWebhookSigning').mockResolvedValue({ secret: 'whsec_new' });

    await user.click(await screen.findByRole('button', { name: 'Replace the secret' }));
    await answerConfirmation();

    expect(rotate).toHaveBeenCalledTimes(1);
    expect(await screen.findByText('whsec_new')).toBeTruthy();
  });

  it('shows the refusal when a replacement fails', async () => {
    const user = userEvent.setup();
    const outcome = createOutcome();
    vi.spyOn(api, 'webhookSigning').mockResolvedValue({
      signed: false,
      since: null,
      readable: true,
    });
    vi.spyOn(api, 'rotateWebhookSigning').mockRejectedValue(new Error('Secret store is read-only'));
    renderWithI18n(WebhookSigning, { props: { outcome } });

    await user.click(await screen.findByRole('button', { name: 'Generate a signing secret' }));

    await waitFor(() => expect(outcome.error).toBe('Secret store is read-only'));
    expect(screen.queryByText(/whsec_/)).toBeNull();
  });

  it('stops signing once asked, and takes the secret just made off the screen', async () => {
    const user = userEvent.setup();
    const status = vi.spyOn(api, 'webhookSigning').mockResolvedValue({
      signed: false,
      since: null,
      readable: true,
    });
    renderWithI18n(WebhookSigning, { props: { outcome: createOutcome() } });
    vi.spyOn(api, 'rotateWebhookSigning').mockResolvedValue({ secret: 'whsec_made' });
    const remove = vi.spyOn(api, 'removeWebhookSigning').mockResolvedValue(undefined);

    status.mockResolvedValue({ signed: true, since: '2026-09-30 12:00:00', readable: true });
    await user.click(await screen.findByRole('button', { name: 'Generate a signing secret' }));
    expect(await screen.findByText('whsec_made')).toBeTruthy();

    status.mockResolvedValue({ signed: false, since: null, readable: true });
    await user.click(await screen.findByRole('button', { name: 'Stop signing' }));
    expect(await answerConfirmation()).toBe(
      'Stop signing notifications? A receiver that checks signatures will refuse them.',
    );

    expect(remove).toHaveBeenCalledTimes(1);
    await waitFor(() => expect(screen.queryByText('whsec_made')).toBeNull());
    // Stop signing goes with the secret, and the button that makes one takes the focus.
    await waitFor(() =>
      expect(document.activeElement).toBe(
        screen.getByRole('button', { name: 'Generate a signing secret' }),
      ),
    );
  });

  it('says why the status could not be read, and asks again on Retry', async () => {
    const user = userEvent.setup();
    const status = vi
      .spyOn(api, 'webhookSigning')
      .mockRejectedValueOnce(new Error('Connection refused'))
      .mockResolvedValue({ signed: false, since: null, readable: true });
    renderWithI18n(WebhookSigning, { props: { outcome: createOutcome() } });

    expect((await screen.findByRole('alert')).textContent).toContain('Connection refused');
    await user.click(screen.getByRole('button', { name: 'Retry' }));

    expect(await screen.findByText('Notifications are not signed.')).toBeTruthy();
    expect(status).toHaveBeenCalledTimes(2);
  });
  it('says when the secret cannot be read', async () => {
    show(true, false);
    expect(
      await screen.findByText(
        "The signing secret cannot be opened with this installation's key, so no notification is sent. Replace the secret.",
      ),
    ).toBeTruthy();
  });

  it('says nothing of the kind while the secret opens', async () => {
    show(true);
    await screen.findByText(/Signed since/);
    expect(
      screen.queryByText(
        "The signing secret cannot be opened with this installation's key, so no notification is sent. Replace the secret.",
      ),
    ).toBeNull();
  });
});
