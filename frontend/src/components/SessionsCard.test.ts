import { describe, it, expect, vi, afterEach } from 'vitest';
import { fireEvent, screen, waitFor, within } from '@testing-library/svelte';

import { renderWithI18n } from '../test/render';
import { answerConfirmation } from '../test/confirm';
import { ApiError, api } from '../api/client';
import type { Session } from '../api/types';
import { createOutcome } from '../lib/outcome.svelte';
import SessionsCard from './SessionsCard.svelte';

/**
 * The live sessions. Ending the one this browser holds signs it out, so the
 * page reloads onto the sign-in rather than failing every request after.
 */

const STRINGS = {
  Sessions: 'Sessions',
  SignOutEverywhere: 'Sign out everywhere',
  NoSessions: 'No browser is signed in.',
  SessionSourceForms: 'Password',
  SessionSourceApikey: 'API key',
  ThisSession: 'This browser',
  EndSession: 'End',
  ConfirmEndSession: 'End this session?',
  ConfirmEndThisSession: 'End the session of this browser?',
  ConfirmEndEverySession: 'End every session?',
  SessionEnded: 'Session ended.',
  Retry: 'Retry',
};

const session = (handle: string, overrides: Partial<Session> = {}): Session => ({
  handle,
  subject: 'admin',
  source: 'forms',
  created_at: '2026-10-01 08:00:00',
  last_used_at: '2026-10-06 09:30:00',
  expires_at: '2026-10-31 08:00:00',
  current: false,
  ...overrides,
});

const HERE = session('aaaa', { current: true });
const THERE = session('bbbb', { source: 'apikey', created_at: '2026-10-02 08:00:00' });

function mount(listed: Session[] = [HERE, THERE]) {
  const list = vi.spyOn(api, 'getSessions').mockResolvedValue(listed);
  const reload = vi.fn();
  vi.stubGlobal('location', { reload });
  const outcome = createOutcome();
  renderWithI18n(SessionsCard, { props: { outcome }, strings: STRINGS });
  return { list, reload, outcome };
}

const ends = () => screen.findAllByRole('button', { name: /^End – / });

afterEach(() => {
  vi.restoreAllMocks();
  vi.unstubAllGlobals();
});

describe('SessionsCard', () => {
  it('names how each session was opened, and the one this browser holds', async () => {
    mount();

    const [here, there] = await ends();
    const row = (button: HTMLElement | undefined) => within(button!.parentElement!);
    expect(row(here).getByText('Password')).toBeInTheDocument();
    expect(row(here).getByText('This browser')).toBeInTheDocument();
    expect(row(there).getByText('API key')).toBeInTheDocument();
    expect(row(there).queryByText('This browser')).toBeNull();
  });

  it('ends another session once confirmed, and lists what is left', async () => {
    const { list, reload, outcome } = mount();
    const end = vi.spyOn(api, 'endSession').mockResolvedValue(undefined);

    const [, there] = await ends();
    await fireEvent.click(there!);
    expect(await answerConfirmation()).toBe('End this session?');

    expect(end).toHaveBeenCalledWith('bbbb');
    await waitFor(() => expect(outcome.notice).toBe('Session ended.'));
    expect(list).toHaveBeenCalledTimes(2);
    expect(reload).not.toHaveBeenCalled();
  });

  it('reloads onto the sign-in once the session of this browser is ended', async () => {
    const { reload } = mount();
    const end = vi.spyOn(api, 'endSession').mockResolvedValue(undefined);

    const [here] = await ends();
    await fireEvent.click(here!);
    expect(await answerConfirmation()).toBe('End the session of this browser?');

    expect(end).toHaveBeenCalledWith('aaaa');
    await waitFor(() => expect(reload).toHaveBeenCalledTimes(1));
  });

  it('ends nothing when the question is declined', async () => {
    mount();
    const end = vi.spyOn(api, 'endSession');
    const all = vi.spyOn(api, 'endEverySession');

    const [, there] = await ends();
    await fireEvent.click(there!);
    await answerConfirmation(null);
    await fireEvent.click(screen.getByRole('button', { name: 'Sign out everywhere' }));
    await answerConfirmation(null);

    expect(end).not.toHaveBeenCalled();
    expect(all).not.toHaveBeenCalled();
  });

  it('signs out everywhere, this browser included', async () => {
    const { reload } = mount();
    const all = vi.spyOn(api, 'endEverySession').mockResolvedValue({ ended: 2 });

    await ends();
    await fireEvent.click(screen.getByRole('button', { name: 'Sign out everywhere' }));
    expect(await answerConfirmation()).toBe('End every session?');

    expect(all).toHaveBeenCalledTimes(1);
    await waitFor(() => expect(reload).toHaveBeenCalledTimes(1));
  });

  it('says a refusal and keeps the page', async () => {
    const { reload, outcome } = mount();
    vi.spyOn(api, 'endEverySession').mockRejectedValue(new ApiError('Not now.', 503, 'busy'));

    await ends();
    await fireEvent.click(screen.getByRole('button', { name: 'Sign out everywhere' }));
    await answerConfirmation();

    await waitFor(() => expect(outcome.error).toBe('Not now.'));
    expect(reload).not.toHaveBeenCalled();
    expect(screen.getByRole('button', { name: 'Sign out everywhere' })).toBeEnabled();
  });

  it('says when no browser is signed in', async () => {
    mount([]);

    expect(await screen.findByText('No browser is signed in.')).toBeInTheDocument();
  });
});
