import { describe, it, expect, vi, afterEach } from 'vitest';
import { screen, waitFor } from '@testing-library/svelte';
import userEvent from '@testing-library/user-event';

import { renderWithI18n } from '../test/render';
import { api } from '../api/client';
import { createOutcome } from '../lib/outcome.svelte';
import NotificationTest from './NotificationTest.svelte';

/**
 * The test message reports through the screen's outcome: delivered, or the
 * server's sentence saying why not.
 */

const STRINGS = {
  NotificationTest: 'Test notification',
  NotificationTestHelp: 'Sends a message to the saved address.',
  SendTestNotification: 'Send a test notification',
  NotificationTestSent: 'The receiver accepted the test notification.',
};

afterEach(() => vi.restoreAllMocks());

describe('NotificationTest', () => {
  it('says the receiver accepted the message', async () => {
    const user = userEvent.setup();
    const outcome = createOutcome();
    const send = vi.spyOn(api, 'sendTestNotification').mockResolvedValue(undefined);
    renderWithI18n(NotificationTest, { props: { outcome }, strings: STRINGS });

    await user.click(screen.getByRole('button', { name: 'Send a test notification' }));

    expect(send).toHaveBeenCalledTimes(1);
    await waitFor(() =>
      expect(outcome.notice).toBe('The receiver accepted the test notification.'),
    );
  });

  it('shows why the message did not arrive, and can be sent again', async () => {
    const user = userEvent.setup();
    const outcome = createOutcome();
    vi.spyOn(api, 'sendTestNotification').mockRejectedValue(
      new Error('The receiver refused the test notification with HTTP 404.'),
    );
    renderWithI18n(NotificationTest, { props: { outcome }, strings: STRINGS });

    const button = screen.getByRole('button', { name: 'Send a test notification' });
    await user.click(button);

    await waitFor(() => expect(outcome.error).toContain('HTTP 404'));
    expect((button as HTMLButtonElement).disabled).toBe(false);
  });
});
