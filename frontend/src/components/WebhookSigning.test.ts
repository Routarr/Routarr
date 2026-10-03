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

const STRINGS = {
  WebhookSigning: 'Webhook signature',
  WebhookSigningHelp: 'Signs every notification.',
  WebhookSignedSince: 'Signed since {since}.',
  WebhookUnsigned: 'Notifications are not signed.',
  GenerateSigningSecret: 'Generate a signing secret',
  ReplaceSigningSecret: 'Replace the secret',
  ConfirmReplaceSigningSecret: 'Replace the signing secret?',
  StopSigning: 'Stop signing',
  ConfirmStopSigning: 'Stop signing notifications?',
  SigningStopped: 'Notifications are no longer signed.',
  ApiKeyMintedOnce: 'Copy it now.',
  SigningSecretUnreadable: 'The secret cannot be read. Replace it.',
  Retry: 'Retry',
};

function show(signed: boolean, readable = true) {
  vi.spyOn(api, 'webhookSigning').mockResolvedValue({
    signed,
    since: signed ? '2026-09-30 12:00:00' : null,
    readable,
  });
  return renderWithI18n(WebhookSigning, {
    props: { outcome: createOutcome() },
    strings: STRINGS,
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
    expect(screen.getByText('Copy it now.')).toBeTruthy();
  });

  it('asks before replacing a secret a receiver may be checking', async () => {
    const user = userEvent.setup();
    show(true);
    const rotate = vi.spyOn(api, 'rotateWebhookSigning').mockResolvedValue({ secret: 'whsec_new' });

    await user.click(await screen.findByRole('button', { name: 'Replace the secret' }));
    expect(await answerConfirmation(null)).toBe('Replace the signing secret?');
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
    renderWithI18n(WebhookSigning, { props: { outcome }, strings: STRINGS });

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
    renderWithI18n(WebhookSigning, { props: { outcome: createOutcome() }, strings: STRINGS });
    vi.spyOn(api, 'rotateWebhookSigning').mockResolvedValue({ secret: 'whsec_made' });
    const remove = vi.spyOn(api, 'removeWebhookSigning').mockResolvedValue(undefined);

    status.mockResolvedValue({ signed: true, since: '2026-09-30 12:00:00', readable: true });
    await user.click(await screen.findByRole('button', { name: 'Generate a signing secret' }));
    expect(await screen.findByText('whsec_made')).toBeTruthy();

    status.mockResolvedValue({ signed: false, since: null, readable: true });
    await user.click(await screen.findByRole('button', { name: 'Stop signing' }));
    expect(await answerConfirmation()).toBe('Stop signing notifications?');

    expect(remove).toHaveBeenCalledTimes(1);
    await waitFor(() => expect(screen.queryByText('whsec_made')).toBeNull());
  });

  it('says why the status could not be read, and asks again on Retry', async () => {
    const user = userEvent.setup();
    const status = vi
      .spyOn(api, 'webhookSigning')
      .mockRejectedValueOnce(new Error('Connection refused'))
      .mockResolvedValue({ signed: false, since: null, readable: true });
    renderWithI18n(WebhookSigning, { props: { outcome: createOutcome() }, strings: STRINGS });

    expect((await screen.findByRole('alert')).textContent).toContain('Connection refused');
    await user.click(screen.getByRole('button', { name: 'Retry' }));

    expect(await screen.findByText('Notifications are not signed.')).toBeTruthy();
    expect(status).toHaveBeenCalledTimes(2);
  });
  /**
   * A secret the installation cannot open sends nothing at all, so the card
   * says so rather than that notifications are signed.
   */
  it('says when the secret cannot be read', async () => {
    show(true, false);
    expect(await screen.findByText('The secret cannot be read. Replace it.')).toBeTruthy();
  });

  it('says nothing of the kind while the secret opens', async () => {
    show(true);
    await screen.findByText(/Signed since/);
    expect(screen.queryByText('The secret cannot be read. Replace it.')).toBeNull();
  });
});
