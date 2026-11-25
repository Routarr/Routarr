import { describe, it, expect, vi, beforeEach, afterEach } from 'vitest';
import { fireEvent, screen } from '@testing-library/svelte';

import { renderWithI18n } from '../test/render';
import { api, ApiError } from '../api/client';
import type { AuthMode } from '../api/types';
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

const show = (install: AuthMode['install'] = 'docker') =>
  renderWithI18n(ApiKeyGate, { props: { install } });

async function send(key: string) {
  const field = await screen.findByLabelText('Routarr API key');
  await fireEvent.input(field, { target: { value: key } });
  await fireEvent.submit(field.closest('form') as HTMLFormElement);
}

describe('ApiKeyGate', () => {
  /** A first visit is often on plain http, where the field selects it for the keyboard. */
  it('offers to copy the command that prints the key', async () => {
    show();

    expect(
      await screen.findByRole('button', {
        name: 'Copy – docker exec routarr cat /config/routarr.api_key',
      }),
    ).toBeInTheDocument();
  });

  it('names the command of the package that installed it', async () => {
    show('rpm');

    expect(await screen.findByText('With the package, this command prints it:')).toBeTruthy();
    expect(
      screen.getByRole('button', {
        name: 'Copy – sudo -u routarr cat /var/lib/routarr/routarr.api_key',
      }),
    ).toBeInTheDocument();
    expect(screen.queryByText(/docker/)).toBeNull();
  });

  /** The installer's data folder lets an administrator in, and no other user. */
  it('names the PowerShell command of the Windows installer', async () => {
    show('windows');

    expect(await screen.findByText(/PowerShell run as administrator/)).toBeTruthy();
    expect(
      screen.getByRole('button', {
        name: 'Copy – Get-Content $env:ProgramData\\Routarr\\routarr.api_key',
      }),
    ).toBeInTheDocument();
    expect(screen.queryByText(/sudo|docker/)).toBeNull();
  });

  /** Where nothing said how it was installed, a command would be a guess that may be wrong. */
  it('names the file alone when the installation is unknown', async () => {
    show(null);

    expect(await screen.findByText(/saved it as routarr.api_key/)).toBeTruthy();
    expect(screen.queryByRole('button', { name: /^Copy/ })).toBeNull();
  });

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
