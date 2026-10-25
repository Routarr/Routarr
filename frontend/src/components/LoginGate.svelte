<script lang="ts">
  import { KeyRound } from '../lib/icons';
  import { ApiError, api } from '../api/client';
  import { takeQueryFlag } from '../api/onboarding';
  import { describeError } from '../lib/async.svelte';
  import { handFocus } from '../lib/focus';
  import { t } from '../lib/i18n.svelte';
  import { router } from '../lib/router.svelte';
  import ErrorBanner from './ErrorBanner.svelte';

  /**
   * Shown in place of the application in `forms` or `oidc` mode when this browser has no
   * session, as `ApiKeyGate` is for a key, the server's `/auth/mode` deciding which. It says
   * where the generated password is.
   */
  let { mode }: { mode: 'forms' | 'oidc' } = $props();

  // The provider redirects back with this when it refused, or when the person
  // did. Without it a failed sign-in returns to the same button with no word.
  // Taken out of the address once read, or a reload says it again.
  const refused = new URLSearchParams(location.search).get('signin') === 'failed';
  takeQueryFlag('signin');
  // The server lands on it only if it is a screen under the mount point.
  const opened = `${router.path}${location.search}${location.hash}`;

  let username = $state('admin');
  let password = $state('');
  let error = $state<string | null>(null);
  /** The password was refused, which the field says as well as the banner. */
  let wrong = $state(false);
  let busy = $state(false);

  // The theme is a server setting behind the gate, so this screen follows the system's, as
  // `ApiKeyGate` does.
  $effect(() => {
    delete document.documentElement.dataset.theme;
  });

  async function submit(event: SubmitEvent) {
    event.preventDefault();
    if (!username.trim() || !password) return;
    const pressed = document.activeElement as HTMLElement | null;
    busy = true;
    error = null;
    wrong = false;
    try {
      await api.login(username, password);
      // A reload rather than a state update: every page in the tree fetched and
      // failed already, and re-mounting them all is what a reload does.
      location.reload();
    } catch (cause) {
      // Its own 401 is the password. Every other 401 is worded as an API key
      // to set, which this screen offers no way to do.
      wrong = cause instanceof ApiError && cause.status === 401;
      error = wrong ? t('SignInWrongCredentials') : describeError(cause);
    } finally {
      busy = false;
      void handFocus(pressed);
    }
  }
</script>

<!-- The page's main landmark: the gate takes the place of the whole shell, and
     so of the shell's own `main`. -->
<main class="gate">
  <form novalidate class="card gate-card" onsubmit={submit}>
    <div class="card-header">
      <h1 class="card-title">
        <KeyRound size={18} />
        {t('SignIn')}
      </h1>
    </div>

    <ErrorBanner
      id="gate-refusal"
      message={error}
      onDismiss={() => ((error = null), (wrong = false))}
    />
    {#if refused}
      <ErrorBanner message={t('SignInRefused')} />
    {/if}

    {#if mode === 'oidc'}
      <p class="text-muted text-base">{t('SignInWithProviderHelp')}</p>
      <!-- An anchor, not a button calling fetch: the browser has to leave this
           origin, and a redirect a script follows is one that can be trapped. -->
      <a class="btn btn-primary" href={api.oidcStartUrl(opened)}>{t('SignInWithProvider')}</a>
    {:else}
      <p class="text-muted text-base">{t('PasswordWhere')}</p>

      <div class="form-group">
        <label class="form-label" for="gate-username">{t('Username')}</label>
        <input
          id="gate-username"
          class="form-input"
          autocomplete="username"
          bind:value={username}
        />
      </div>

      <div class="form-group">
        <label class="form-label" for="gate-password">{t('Password')}</label>
        <!-- svelte-ignore a11y_autofocus -->
        <input
          id="gate-password"
          type="password"
          class="form-input"
          autocomplete="current-password"
          aria-invalid={wrong ? 'true' : undefined}
          aria-describedby={wrong ? 'gate-refusal' : undefined}
          bind:value={password}
          autofocus
        />
      </div>

      <button type="submit" class="btn btn-primary" disabled={busy || !password}>
        {busy ? t('SigningIn') : t('SignIn')}
      </button>
    {/if}
  </form>
</main>
