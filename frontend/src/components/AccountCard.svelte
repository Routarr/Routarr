<script lang="ts">
  import { Lock } from '../lib/icons';
  import { api } from '../api/client';
  import { t } from '../lib/i18n.svelte';
  import type { Outcome } from '../lib/outcome.svelte';
  import WarningBanner from './WarningBanner.svelte';

  /**
   * The `forms` account's password, changed against the current one, and every
   * key revoked with it when the password is changed because it leaked.
   */
  let { outcome }: { outcome: Outcome } = $props();

  /** The server's minimum, which `PUT /auth/password` refuses below. */
  const SHORTEST = 12;

  let current = $state('');
  let next = $state('');
  let repeated = $state('');
  let revokeKeys = $state(false);
  let saving = $state(false);
  /** The API key a revocation replaced the old one with, shown this once. */
  let replaced = $state<string | null>(null);

  const tooShort = $derived(next.length > 0 && [...next].length < SHORTEST);
  const mismatch = $derived(repeated.length > 0 && repeated !== next);
  const ready = $derived(current.length > 0 && next.length > 0 && !tooShort && repeated === next);

  async function submit(event: SubmitEvent) {
    event.preventDefault();
    if (!ready) return;
    saving = true;
    try {
      const changed = await api.changePassword(current, next, revokeKeys);
      replaced = changed.api_key;
      current = '';
      next = '';
      repeated = '';
      outcome.succeed(t(revokeKeys ? 'PasswordChangedKeysRevoked' : 'PasswordChanged'));
      revokeKeys = false;
    } catch (cause) {
      outcome.fail(cause);
    } finally {
      saving = false;
    }
  }
</script>

<div class="card">
  <div class="card-header">
    <div>
      <h2 class="card-title flex items-center gap-2">
        <Lock size={18} aria-hidden="true" />
        {t('Account')}
      </h2>
      <p class="card-note">{t('AccountHelp')}</p>
    </div>
  </div>

  {#if replaced}
    <div class="mb-3">
      <WarningBanner message={t('ApiKeyMintedOnce')} />
      <code class="mono secret-once">{replaced}</code>
    </div>
  {/if}

  <form novalidate onsubmit={submit}>
    <div class="form-group">
      <label class="form-label" for="account-current">{t('CurrentPassword')}</label>
      <input
        id="account-current"
        type="password"
        class="form-input"
        autocomplete="current-password"
        bind:value={current}
      />
    </div>
    <div class="form-group">
      <label class="form-label" for="account-new">{t('NewPassword')}</label>
      <input
        id="account-new"
        type="password"
        class="form-input"
        autocomplete="new-password"
        aria-describedby="account-new-help"
        aria-invalid={tooShort}
        bind:value={next}
      />
      <p id="account-new-help" class="form-hint">
        {t('PasswordMinimum', { min: SHORTEST })}
      </p>
    </div>
    <div class="form-group">
      <label class="form-label" for="account-repeat">{t('RepeatPassword')}</label>
      <input
        id="account-repeat"
        type="password"
        class="form-input"
        autocomplete="new-password"
        aria-describedby={mismatch ? 'account-repeat-error' : undefined}
        aria-invalid={mismatch}
        bind:value={repeated}
      />
      {#if mismatch}
        <p id="account-repeat-error" class="field-error">{t('PasswordsDiffer')}</p>
      {/if}
    </div>
    <label class="check-option mb-3">
      <input type="checkbox" bind:checked={revokeKeys} />
      <span>{t('RevokeEveryKey')}<span class="form-hint">{t('RevokeEveryKeyHelp')}</span></span>
    </label>
    <button type="submit" class="btn btn-primary" disabled={!ready || saving}>
      {t('ChangePassword')}
    </button>
  </form>
</div>
