import { describe, it, expect, vi, beforeEach, afterEach } from 'vitest';
import { fireEvent, screen } from '@testing-library/svelte';

import { renderWithI18n } from '../test/render';
import { api, ApiError } from '../api/client';
import ApiKeyGate from './ApiKeyGate.svelte';

const reload = vi.fn();

beforeEach(() => {
  // `location.reload` would navigate for real. The assertion is that it is
  // called, not what it does.
  Object.defineProperty(window, 'location', {
    value: { ...window.location, reload },
    configurable: true,
  });
});

afterEach(() => vi.restoreAllMocks());

const show = () => renderWithI18n(ApiKeyGate);

async function send(key: string) {
  const field = await screen.findByLabelText('Routarr API key');
  await fireEvent.input(field, { target: { value: key } });
  await fireEvent.submit(field.closest('form') as HTMLFormElement);
}

describe('ApiKeyGate', () => {
  it('refuses to submit an empty key', async () => {
    show();

    const signIn = await screen.findByRole('button', { name: 'Sign in' });
    expect((signIn as HTMLButtonElement).disabled).toBe(true);
  });

  it('exchanges the key for a session, keeps no copy and remounts the application', async () => {
    const exchanged = vi.spyOn(api, 'keySession').mockResolvedValue({ ok: true });
    show();

    await send(' a-real-key ');

    expect(exchanged).toHaveBeenCalledWith('a-real-key');
    expect(localStorage.getItem('routarr.apiKey')).toBeNull();
    await vi.waitFor(() => expect(reload).toHaveBeenCalled());
  });

  it('says a refused key was refused, and stays', async () => {
    vi.spyOn(api, 'keySession').mockRejectedValue(new ApiError('', 401, 'unauthorized'));
    show();

    expect(screen.queryByText('That key was refused. Check it and try again.')).toBeNull();
    await send('the-wrong-key');

    expect(await screen.findByText('That key was refused. Check it and try again.')).toBeTruthy();
    expect(reload).not.toHaveBeenCalled();
  });
});
