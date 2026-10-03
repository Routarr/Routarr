<script lang="ts">
  import { KeyRound } from '../lib/icons';
  import { api } from '../api/client';
  import { createAsync } from '../lib/async.svelte';
  import { i18n, t } from '../lib/i18n.svelte';
  import type { Outcome } from '../lib/outcome.svelte';
  import { formatTimestamp } from '../api/format';
  import { askConfirmation } from '../lib/confirm.svelte';
  import ErrorBanner from './ErrorBanner.svelte';
  import WarningBanner from './WarningBanner.svelte';

  /**
   * The secret the notification webhook is signed with.
   *
   * Separate from the settings form because making one is an action, not a
   * value to save, and the secret is shown once, as it is made.
   */
  let { outcome }: { outcome: Outcome } = $props();

  const signing = createAsync((signal) => api.webhookSigning(signal));
  let minted = $state<string | null>(null);

  async function rotate() {
    const replacing = signing.data?.signed ?? false;
    if (
      replacing &&
      !(await askConfirmation(t('ConfirmReplaceSigningSecret'), 'ReplaceSigningSecret'))
    )
      return;
    try {
      minted = (await api.rotateWebhookSigning()).secret;
      outcome.clear();
      await signing.reload();
    } catch (cause) {
      outcome.fail(cause);
    }
  }

  async function stop() {
    if (!(await askConfirmation(t('ConfirmStopSigning'), 'StopSigning'))) return;
    try {
      await api.removeWebhookSigning();
      minted = null;
      outcome.succeed(t('SigningStopped'));
      await signing.reload();
    } catch (cause) {
      outcome.fail(cause);
    }
  }
</script>

<div class="card">
  <div class="card-header">
    <div>
      <h2 class="card-title flex items-center gap-2">
        <KeyRound size={18} aria-hidden="true" />
        {t('WebhookSigning')}
      </h2>
      <p class="card-note">{t('WebhookSigningHelp')}</p>
    </div>
  </div>

  {#if minted}
    <!-- The one moment the secret is readable. `mono` carries the
         `direction: ltr` it needs in a right-to-left page. -->
    <div class="mb-3">
      <WarningBanner message={t('ApiKeyMintedOnce')} />
      <code class="mono secret-once">{minted}</code>
    </div>
  {/if}

  {#if signing.error}
    <div class="mb-3">
      <ErrorBanner message={signing.error} onRetry={() => void signing.reload()} />
    </div>
  {/if}

  {#if signing.data}
    {#if signing.data.signed && !signing.data.readable}
      <!-- Nothing is sent while the secret cannot be opened, which reads as
           signing from the date alone. -->
      <div class="mb-3">
        <WarningBanner message={t('SigningSecretUnreadable')} />
      </div>
    {/if}
    <p class="text-muted text-sm mb-3">
      {signing.data.signed && signing.data.since
        ? t('WebhookSignedSince', { since: formatTimestamp(signing.data.since, i18n.language) })
        : t('WebhookUnsigned')}
    </p>
    <div class="flex gap-2">
      <button type="button" class="btn btn-secondary" onclick={() => void rotate()}>
        {t(signing.data.signed ? 'ReplaceSigningSecret' : 'GenerateSigningSecret')}
      </button>
      {#if signing.data.signed}
        <button type="button" class="btn btn-danger" onclick={() => void stop()}>
          {t('StopSigning')}
        </button>
      {/if}
    </div>
  {/if}
</div>
