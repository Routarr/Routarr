<script lang="ts">
  import { KeyRound } from '../lib/icons';
  import { api } from '../api/client';
  import { describeError } from '../lib/async.svelte';
  import { t } from '../lib/i18n.svelte';
  import ErrorBanner from './ErrorBanner.svelte';

  /**
   * Shown instead of the application when the server runs in `forms` mode and
   * this browser has no live session.
   *
   * The sibling of `ApiKeyGate`, and for the same reason: mounting the pages
   * behind a refusal produces a failed request each and reads as a broken
   * install. Which of the two appears is the server's answer to `/auth/mode`,
   * not a guess.
   *
   * It says where the generated password is, because "sign in" is useless to
   * someone who has never been given a credential.
   */
  let { mode }: { mode: 'forms' | 'oidc' } = $props();

  // The provider redirects back with this when it refused, or when the person
  // did. Without it a failed sign-in returns to the same button with no word.
  const refused = new URLSearchParams(location.search).get('signin') === 'failed';

  let username = $state('admin');
  let password = $state('');
  let error = $state<string | null>(null);
  let busy = $state(false);

  // The theme is a server setting behind the gate, so this one screen follows
  // the operating system — the same choice `ApiKeyGate` makes, for the same
  // reason: guessing dark would look wrong on a light desktop.
  $effect(() => {
    delete document.documentElement.dataset.theme;
  });

  async function submit(event: SubmitEvent) {
    event.preventDefault();
    if (!username.trim() || !password) return;
    busy = true;
    error = null;
    try {
      await api.login(username, password);
      // A reload rather than a state update: every page in the tree fetched and
      // failed already, and re-mounting them all is what a reload does.
      location.reload();
    } catch (cause) {
      error = describeError(cause);
    } finally {
      busy = false;
    }
  }
</script>

<div class="gate">
  <form novalidate class="card gate-card" onsubmit={submit}>
    <div class="card-header">
      <h1 class="card-title">
        <KeyRound size={18} />
        {t('SignIn')}
      </h1>
    </div>

    <ErrorBanner message={error} onDismiss={() => (error = null)} />
    {#if refused}
      <ErrorBanner message={t('SignInRefused')} />
    {/if}

    {#if mode === 'oidc'}
      <p class="text-muted text-base">{t('SignInWithProviderHelp')}</p>
      <!-- An anchor, not a button calling fetch: the browser has to leave this
           origin, and a redirect a script follows is one that can be trapped. -->
      <a class="btn btn-primary" href={api.oidcStartUrl()}>{t('SignInWithProvider')}</a>
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
          bind:value={password}
          autofocus
        />
      </div>

      <button type="submit" class="btn btn-primary" disabled={busy || !password}>
        {busy ? t('SigningIn') : t('SignIn')}
      </button>
    {/if}
  </form>
</div>
