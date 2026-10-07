import { describe, it, expect, vi, afterEach } from 'vitest';
import { waitFor } from '@testing-library/svelte';

import { api, ApiError } from '../api/client';
import type { AuthMode } from '../api/types';
import { answerConfirmation } from '../test/confirm';
import { withBase } from '../test/base';
import { proofRequest, settleProof, signInAgainUrl, withProof } from './proof.svelte';
import { router } from './router.svelte';

/**
 * The proof a session gives before a key is made or withdrawn. Sent without
 * it, the server refuses the write, and a proof asked for in the wrong mode is
 * a password prompt nobody can answer.
 */

const inMode = (mode: AuthMode['mode']) =>
  vi.spyOn(api, 'authMode').mockResolvedValue({
    mode,
    api_key_configured: true,
    api_key_pinned: false,
  });

/** Answer the pending proof, and hand back what it asked for. */
async function answerProof(value: string | null): Promise<string> {
  await waitFor(() => expect(proofRequest.request).not.toBeNull());
  const asked = proofRequest.request?.asked ?? '';
  settleProof(value);
  return asked;
}

afterEach(() => {
  settleProof(null);
  vi.restoreAllMocks();
  vi.unstubAllGlobals();
  withBase(null);
});

describe('withProof', () => {
  it.each([
    ['forms', 'password', { current_password: 'typed' }],
    ['apikey', 'key', { current_key: 'typed' }],
  ] as const)('asks a %s session for its %s and sends it', async (mode, asked, sent) => {
    inMode(mode);
    const send = vi.fn().mockResolvedValue('written');

    const result = withProof(send);
    expect(await answerProof('typed')).toBe(asked);

    expect(await result).toBe('written');
    expect(send).toHaveBeenCalledWith(sent);
  });

  /** The provider proves an OpenID Connect session, when the server asks it to. */
  it.each(['oidc', 'none', 'external'] as const)('asks a %s session nothing', async (mode) => {
    inMode(mode);
    const send = vi.fn().mockResolvedValue('written');

    expect(await withProof(send)).toBe('written');
    expect(send).toHaveBeenCalledWith({});
    expect(proofRequest.request).toBeNull();
  });

  it('sends nothing once the proof is declined', async () => {
    inMode('forms');
    const send = vi.fn();

    const result = withProof(send);
    await answerProof(null);

    expect(await result).toBeNull();
    expect(send).not.toHaveBeenCalled();
  });

  /**
   * A second write asking while the first waits declines the first, which
   * would otherwise wait forever on a dialog that now answers the second.
   */
  it('declines the proof still pending when another is asked', async () => {
    inMode('forms');
    const first = vi.fn();
    const second = vi.fn().mockResolvedValue('second');

    const pending = withProof(first);
    await waitFor(() => expect(proofRequest.request).not.toBeNull());
    const next = withProof(second);

    expect(await pending).toBeNull();
    await answerProof('typed');
    expect(await next).toBe('second');
    expect(first).not.toHaveBeenCalled();
  });

  it('passes on a refusal that is not a request to sign in again', async () => {
    inMode('oidc');
    const refused = new ApiError('No.', 403, 'forbidden');

    await expect(withProof(() => Promise.reject(refused))).rejects.toBe(refused);
  });

  /**
   * An OpenID Connect sign-in older than the server accepts: the provider signs
   * the person in again and brings them back to the screen they were on.
   */
  it('sends a stale OpenID Connect session to the provider once agreed', async () => {
    inMode('oidc');
    const assign = vi.fn();
    vi.stubGlobal('location', { assign });
    router.path = '/applications';
    const stale = new ApiError('Sign in again first.', 403, 'reauthentication_required');

    const result = withProof(() => Promise.reject(stale));
    expect(await answerConfirmation()).toBe('Sign in again first.');

    expect(await result).toBeNull();
    expect(assign).toHaveBeenCalledWith(signInAgainUrl('/applications'));
  });

  it('stays on the screen when signing in again is declined', async () => {
    inMode('oidc');
    const assign = vi.fn();
    vi.stubGlobal('location', { assign });
    const stale = new ApiError('Sign in again first.', 403, 'reauthentication_required');

    const result = withProof(() => Promise.reject(stale));
    await answerConfirmation(null);

    expect(await result).toBeNull();
    expect(assign).not.toHaveBeenCalled();
  });
});

describe('signInAgainUrl', () => {
  /** `max_age=0` is what makes the provider ask again rather than wave the session through. */
  it('asks the provider for a new sign-in and names the screen to come back to', () => {
    const url = new URL(signInAgainUrl('/settings'), 'http://routarr.test');

    expect(url.pathname).toBe('/api/v1/auth/oidc/start');
    expect(url.searchParams.get('max_age')).toBe('0');
    expect(url.searchParams.get('return')).toBe('/settings');
  });

  it('goes through the mount point a reverse proxy adds', () => {
    withBase('/routarr/');

    expect(signInAgainUrl('/settings')).toMatch(/^\/routarr\/api\/v1\/auth\/oidc\/start\?/);
  });
});
