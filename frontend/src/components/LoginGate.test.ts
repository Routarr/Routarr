import { describe, it, expect, vi, afterEach } from 'vitest';
import { fireEvent, screen, waitFor } from '@testing-library/svelte';
import userEvent from '@testing-library/user-event';

import { renderWithI18n } from '../test/render';
import { ApiError, api } from '../api/client';
import LoginGate from './LoginGate.svelte';

/**
 * The screen a `forms` installation opens on. Its whole job is to be usable by
 * someone who has never been handed a credential, which is why it says where
 * the generated password is rather than assuming they know.
 */

const STRINGS = {
  SignIn: 'Sign in',
  SigningIn: 'Signing in…',
  Username: 'Username',
  Password: 'Password',
  PasswordWhere: 'The password was generated at first start.',
  SignInWithProvider: 'Sign in with your provider',
  SignInWithProviderHelp: 'Routarr does not hold your password.',
  Unauthorized: 'Unauthorized',
  Dismiss: 'Dismiss',
};

afterEach(() => vi.restoreAllMocks());

describe('LoginGate', () => {
  it('says where the password is, since nobody chose it', () => {
    renderWithI18n(LoginGate, { props: { mode: 'forms' }, strings: STRINGS });

    expect(screen.getByText('The password was generated at first start.')).toBeInTheDocument();
    // One account, so the name is filled in and only the password is asked for.
    expect(screen.getByLabelText('Username')).toHaveValue('admin');
  });

  it('signs in with what was typed', async () => {
    const login = vi.spyOn(api, 'login').mockResolvedValue({ username: 'admin' });
    renderWithI18n(LoginGate, { props: { mode: 'forms' }, strings: STRINGS });

    await userEvent.type(screen.getByLabelText('Password'), 'deadbeef');
    await userEvent.click(screen.getByRole('button', { name: 'Sign in' }));

    expect(login).toHaveBeenCalledWith('admin', 'deadbeef');
  });

  /// A refusal has to reach the screen: swallowed, a wrong password looks like
  /// a button that does nothing.
  it('shows the refusal rather than swallowing it', async () => {
    vi.spyOn(api, 'login').mockRejectedValue(
      new ApiError('Wrong username or password.', 401, 'unauthorized'),
    );
    renderWithI18n(LoginGate, { props: { mode: 'forms' }, strings: STRINGS });

    await userEvent.type(screen.getByLabelText('Password'), 'nope');
    await userEvent.click(screen.getByRole('button', { name: 'Sign in' }));

    await waitFor(() => expect(screen.getByRole('alert')).toBeInTheDocument());
  });

  /// Nothing to type: the provider decides, and the browser has to leave this
  /// origin, so the control is a link and not a button calling fetch.
  it('offers the provider instead of a password field in oidc mode', () => {
    renderWithI18n(LoginGate, { props: { mode: 'oidc' }, strings: STRINGS });

    const link = screen.getByRole('link', { name: 'Sign in with your provider' });
    expect(link).toHaveAttribute('href', expect.stringContaining('/auth/oidc/start'));
    expect(screen.queryByLabelText('Password')).toBeNull();
  });

  it('does not submit an empty password', async () => {
    const login = vi.spyOn(api, 'login');
    renderWithI18n(LoginGate, { props: { mode: 'forms' }, strings: STRINGS });

    const button = screen.getByRole('button', { name: 'Sign in' });
    expect(button).toBeDisabled();
    // And not only the button: a submit that reaches the handler anyway — a
    // form driven by a script, or a browser that ignores a disabled default
    // button — sends nothing either.
    await fireEvent.submit(button.closest('form') as HTMLFormElement);
    expect(login).not.toHaveBeenCalled();
  });
});
