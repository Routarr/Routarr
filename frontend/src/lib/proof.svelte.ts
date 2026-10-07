import { api, ApiError } from '../api/client';
import type { Proof } from '../api/types';
import { askConfirmation } from './confirm.svelte';
import { basePath } from '../api/basePath';
import { router } from './router.svelte';
import { t } from './i18n.svelte';

/**
 * The proof a session gives before it makes or withdraws a key: the current
 * password in `forms`, the API key in `apikey`. An OpenID Connect session
 * proves nothing here, the server asking the provider instead when the sign-in
 * is too old (`reauthentication_required`).
 *
 * One dialog, mounted once by `Layout`, as the confirmation is.
 */

/** What the dialog asks for. */
export type Asked = 'password' | 'key' | 'passphrase';

/** `note`, when there is one, is said in place of the help: why it asks again. */
type Request = { asked: Asked; note?: string; resolve: (value: string | null) => void };

const state = $state<{ request: Request | null }>({ request: null });

export const proofRequest = {
  get request(): Request | null {
    return state.request;
  },
};

/** Close the dialog with what was typed, or `null` for Cancel and Escape. */
export function settleProof(value: string | null): void {
  const pending = state.request;
  state.request = null;
  pending?.resolve(value);
}

function ask(asked: Asked, note?: string): Promise<string | null> {
  state.request?.resolve(null);
  return new Promise((resolve) => {
    state.request = { asked, note, resolve };
  });
}

/**
 * The passphrase a sealed archive was taken with, through the same dialog.
 * `note` says why it is asked again, after one that opened nothing.
 */
export function askPassphrase(note?: string): Promise<string | null> {
  return ask('passphrase', note);
}

/** Where the provider signs the person in again and sends them back here. */
export function signInAgainUrl(screen: string): string {
  return `${basePath()}/api/v1/auth/oidc/start?max_age=0&return=${encodeURIComponent(screen)}`;
}

/**
 * Send a write that touches a key, with the proof the mode asks for. `null` is
 * a proof declined, or a sign-in at the provider the person chose to make
 * first, which leaves the page.
 */
export async function withProof<T>(send: (proof: Proof) => Promise<T>): Promise<T | null> {
  const mode = (await api.authMode()).mode;
  let proof: Proof = {};
  if (mode === 'forms' || mode === 'apikey') {
    const typed = await ask(mode === 'forms' ? 'password' : 'key');
    if (typed === null) return null;
    proof = mode === 'forms' ? { current_password: typed } : { current_key: typed };
  }
  try {
    return await send(proof);
  } catch (cause) {
    if (cause instanceof ApiError && cause.kind === 'reauthentication_required') {
      if (await askConfirmation(cause.message || t('SignInAgainHelp'), 'SignInAgain', false)) {
        location.assign(signInAgainUrl(router.path));
      }
      return null;
    }
    throw cause;
  }
}
