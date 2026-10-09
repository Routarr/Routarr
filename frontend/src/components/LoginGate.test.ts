import { describe, it, expect, vi, afterEach } from 'vitest';
import { fireEvent, screen, waitFor } from '@testing-library/svelte';
import userEvent from '@testing-library/user-event';

import { renderWithI18n } from '../test/render';
import { ApiError, api } from '../api/client';
import { interceptLinks, router } from '../lib/router.svelte';
import { dropFocus } from '../test/focus';
import { click } from '../test/links';
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

afterEach(() => {
  vi.restoreAllMocks();
  vi.unstubAllGlobals();
});

describe('LoginGate', () => {
  it('says where the password is, since nobody chose it', () => {
    renderWithI18n(LoginGate, { props: { mode: 'forms' }, strings: STRINGS });

    expect(screen.getByText('The password was generated at first start.')).toBeInTheDocument();
    // One account, so the name is filled in and only the password is asked for.
    expect(screen.getByLabelText('Username')).toHaveValue('admin');
  });

  /**
   * Then a reload rather than a state update: every screen behind the gate
   * fetched and failed already. jsdom reloads nothing, so the page's own
   * `location` stands in for the browser's.
   */
  it('signs in with what was typed, then reloads the page', async () => {
    const login = vi.spyOn(api, 'login').mockResolvedValue({ username: 'admin' });
    const reload = vi.fn();
    vi.stubGlobal('location', { search: '', reload });
    renderWithI18n(LoginGate, { props: { mode: 'forms' }, strings: STRINGS });

    await userEvent.type(screen.getByLabelText('Password'), 'deadbeef');
    await userEvent.click(screen.getByRole('button', { name: 'Sign in' }));

    expect(login).toHaveBeenCalledWith('admin', 'deadbeef');
    await waitFor(() => expect(reload).toHaveBeenCalledTimes(1));
  });

  /**
   * A refusal has to reach the screen: swallowed, a wrong password looks like
   * a button that does nothing. Worded as every other 401 is, it asks for an
   * API key this screen offers no way to set.
   */
  it('says a wrong password is wrong, rather than swallowing it', async () => {
    vi.spyOn(api, 'login').mockRejectedValue(
      new ApiError('Wrong username or password.', 401, 'unauthorized'),
    );
    renderWithI18n(LoginGate, {
      props: { mode: 'forms' },
      strings: { ...STRINGS, SignInWrongCredentials: 'The username or the password is wrong.' },
    });

    await userEvent.type(screen.getByLabelText('Password'), 'nope');
    await userEvent.click(screen.getByRole('button', { name: 'Sign in' }));

    await waitFor(() =>
      expect(screen.getByRole('alert')).toHaveTextContent('The username or the password is wrong.'),
    );
  });

  /** Sign in turns disabled while it asks, which drops its focus. */
  it('gives the focus back to Sign in once a refusal is shown', async () => {
    let refuse = () => {};
    vi.spyOn(api, 'login').mockReturnValue(
      new Promise((_, reject) => (refuse = () => reject(new Error('Refused')))),
    );
    renderWithI18n(LoginGate, { props: { mode: 'forms' }, strings: STRINGS });

    await userEvent.type(screen.getByLabelText('Password'), 'nope');
    const button = screen.getByRole('button', { name: 'Sign in' });
    await userEvent.click(button);
    expect(button).toBeDisabled();
    dropFocus();
    refuse();

    await waitFor(() => expect(document.activeElement).toBe(button));
  });

  /**
   * Nothing to type: the provider decides, and the browser has to leave this
   * origin, so the control is a link and not a button calling fetch.
   */
  it('offers the provider instead of a password field in oidc mode', () => {
    renderWithI18n(LoginGate, { props: { mode: 'oidc' }, strings: STRINGS });

    const link = screen.getByRole('link', { name: 'Sign in with your provider' });
    expect(link).toHaveAttribute('href', expect.stringContaining('/auth/oidc/start'));
    expect(screen.queryByLabelText('Password')).toBeNull();
  });

  /**
   * The gate renders inside `App`, whose router listens to every click on the
   * page. The link is only a way out if that listener lets it go.
   */
  it('lets the provider link leave the page with the router listening', async () => {
    const stop = interceptLinks();
    renderWithI18n(LoginGate, { props: { mode: 'oidc' }, strings: STRINGS });

    const taken = click(screen.getByRole('link', { name: 'Sign in with your provider' }));

    stop();
    expect(taken, 'the router took the click').toBe(false);
    expect(router.path).not.toContain('/auth/oidc/start');
  });

  /**
   * The provider's refusal is said once. Left in the address, a reload or a
   * bookmark says it again about a sign-in nobody attempted.
   */
  it('says the provider refused, and takes the word out of the address', () => {
    window.history.replaceState({}, '', '/?signin=failed');
    renderWithI18n(LoginGate, {
      props: { mode: 'oidc' },
      strings: { ...STRINGS, SignInRefused: 'The provider refused the sign-in' },
    });

    expect(screen.getByText('The provider refused the sign-in')).toBeTruthy();
    expect(window.location.search).toBe('');
  });

  it('does not submit an empty password', async () => {
    const login = vi.spyOn(api, 'login');
    renderWithI18n(LoginGate, { props: { mode: 'forms' }, strings: STRINGS });

    const button = screen.getByRole('button', { name: 'Sign in' });
    expect(button).toBeDisabled();
    // And not only the button: a submit that reaches the handler anyway (a
    // form driven by a script, or a browser that ignores a disabled default
    // button) sends nothing either.
    await fireEvent.submit(button.closest('form') as HTMLFormElement);
    expect(login).not.toHaveBeenCalled();
  });
});
