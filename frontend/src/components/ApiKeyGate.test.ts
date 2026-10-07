import { describe, it, expect, vi, beforeEach, afterEach } from 'vitest';
import { fireEvent, screen } from '@testing-library/svelte';

import { renderWithI18n } from '../test/render';
import { api, ApiError } from '../api/client';
import ApiKeyGate from './ApiKeyGate.svelte';

/**
 * A key is generated at first start, so a browser without one is the *ordinary*
 * first visit. Mounting the shell behind it instead reads as a broken install:
 * a failing request per page, every page empty, and an "Unauthorized" badge
 * whose remedy the user would have to already know.
 */

const STRINGS = {
  ApiKeyRequired: 'This Routarr needs an API key',
  ApiKeyRejected: 'That key was refused',
  ApiKeyGeneratedFile: 'Saved as {file}',
  ApiKeyDockerCommand: 'With the Docker image:',
  ApiKeyChooseOwn: 'Or set {variable}',
  RoutarrApiKey: 'Routarr API key',
  SignIn: 'Sign in',
};

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

const show = () => renderWithI18n(ApiKeyGate, { strings: STRINGS });

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

  /**
   * The key is sent once for a session cookie, and kept nowhere a script on
   * the page could read it.
   */
  it('exchanges the key for a session, keeps no copy and remounts the application', async () => {
    const exchanged = vi.spyOn(api, 'keySession').mockResolvedValue({ ok: true });
    show();

    await send(' a-real-key ');

    expect(exchanged).toHaveBeenCalledWith('a-real-key');
    expect(localStorage.getItem('routarr.apiKey')).toBeNull();
    // Every page in the tree has already fetched and failed, and re-mounting
    // them all is exactly what a reload does.
    await vi.waitFor(() => expect(reload).toHaveBeenCalled());
  });

  /**
   * A refused key is said to be wrong: without it, pasting a bad key returns
   * the same screen and the user pastes it again.
   */
  it('says a refused key was refused, and stays', async () => {
    vi.spyOn(api, 'keySession').mockRejectedValue(new ApiError('', 401, 'unauthorized'));
    show();

    expect(screen.queryByText('That key was refused')).toBeNull();
    await send('the-wrong-key');

    expect(await screen.findByText('That key was refused')).toBeTruthy();
    expect(reload).not.toHaveBeenCalled();
  });
});
