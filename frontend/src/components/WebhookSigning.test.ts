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
};

function show(signed: boolean) {
  vi.spyOn(api, 'webhookSigning').mockResolvedValue({
    signed,
    since: signed ? '2026-09-30 12:00:00' : null,
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

  it('stops signing once asked', async () => {
    const user = userEvent.setup();
    show(true);
    const remove = vi.spyOn(api, 'removeWebhookSigning').mockResolvedValue(undefined);

    await user.click(await screen.findByRole('button', { name: 'Stop signing' }));
    expect(await answerConfirmation()).toBe('Stop signing notifications?');
    await waitFor(() => expect(remove).toHaveBeenCalled());
  });
});
