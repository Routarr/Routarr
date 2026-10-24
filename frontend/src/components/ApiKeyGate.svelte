<script lang="ts">
  import { KeyRound } from '../lib/icons';
  import { api, ApiError } from '../api/client';
  import { describeError } from '../lib/async.svelte';
  import { t } from '../lib/i18n.svelte';
  import ErrorBanner from './ErrorBanner.svelte';

  /**
   * Shown in place of the application when the server wants a key this browser lacks, the
   * ordinary first visit. It names the key's file, which is on the volume, where the log line
   * goes with the container. The key is traded once for a session cookie no script can read.
   */
  let key = $state('');
  let error = $state<string | null>(null);
  let busy = $state(false);

  // The container name of the README and docker-compose.yml, and the image's data directory.
  // `scripts/smoke-image.sh` runs this line against the built image.
  const READ_KEY_COMMAND = 'docker exec routarr cat /config/routarr.api_key';

  // The theme is a server setting the server tells nobody without a key, so this screen
  // follows the system's.
  $effect(() => {
    delete document.documentElement.dataset.theme;
  });

  async function submit(event: SubmitEvent) {
    event.preventDefault();
    if (!key.trim()) return;
    busy = true;
    error = null;
    try {
      await api.keySession(key.trim());
      // A reload rather than a state update: every page in the tree fetched
      // and failed already, and re-mounting them all is what a reload does.
      location.reload();
    } catch (cause) {
      // A wrong key, said so: without it, pasting a bad key returns the same
      // screen and the user pastes it again.
      error =
        cause instanceof ApiError && cause.status === 401
          ? t('ApiKeyRejected')
          : describeError(cause);
    } finally {
      busy = false;
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
        {t('ApiKeyRequired')}
      </h1>
    </div>

    <ErrorBanner message={error} onDismiss={() => (error = null)} />

    <p class="text-muted text-base mb-3">
      {t('ApiKeyGeneratedFile', { file: 'routarr.api_key' })}
    </p>
    <p class="text-muted text-base">{t('ApiKeyDockerCommand')}</p>
    <code class="mono gate-command">{READ_KEY_COMMAND}</code>

    <div class="form-group">
      <label class="form-label" for="gate-api-key">{t('RoutarrApiKey')}</label>
      <!-- The one field on the screen, and the only reason the screen is here. -->
      <!-- svelte-ignore a11y_autofocus -->
      <input
        id="gate-api-key"
        type="password"
        class="form-input"
        placeholder={t('ApiKeyPlaceholder')}
        bind:value={key}
        autofocus
      />
    </div>

    <button type="submit" class="btn btn-primary" disabled={!key.trim() || busy}>
      {t('SignIn')}
    </button>

    <p class="text-muted text-sm mt-3">
      {t('ApiKeyChooseOwn', { variable: 'ROUTARR_API_KEY' })}
    </p>
  </form>
</main>
