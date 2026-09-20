<script lang="ts">
  import { KeyRound } from '../lib/icons';
  import { getApiKey, setApiKey } from '../api/client';
  import { t } from '../lib/i18n.svelte';
  import ErrorBanner from './ErrorBanner.svelte';

  /**
   * Shown instead of the application when the server demands a key this browser
   * has not got.
   *
   * Routarr generates an API key at first start, so this is the *normal* first
   * visit, not an error state. Mounting the pages behind it instead reads as a
   * broken install: a failing request per page, every page empty, and a small
   * "Unauthorized" badge whose remedy — open Settings, paste the key — the user
   * would have to already know.
   *
   * It says where the key is, because "enter your API key" is useless to someone
   * who has never seen one. The file, not the log: the log line is printed once
   * and goes with the container the first time an image update recreates it,
   * while the file is on the volume.
   */
  let key = $state('');

  // The container name the README and docker-compose.yml give it, and the data
  // directory the image sets. `scripts/smoke-image.sh` reads this line and runs
  // it against the built image, so an instruction that stopped working fails
  // the image build rather than a first visit.
  const READ_KEY_COMMAND = 'docker exec routarr cat /data/routarr.api_key';

  // A key is already stored and the server still refused it: that is a *wrong*
  // key, not a missing one. Without saying so, pasting a bad key returns the
  // same blank screen and the user pastes it again.
  const rejected = getApiKey().trim().length > 0;

  // The theme is a server setting, and the server will not answer until there
  // is a key — so this one screen follows the operating system instead. It is
  // the only honest option: guessing dark would look wrong on a light desktop,
  // and the preference this reads is the user's own.
  $effect(() => {
    delete document.documentElement.dataset.theme;
  });

  function submit(event: SubmitEvent) {
    event.preventDefault();
    if (!key.trim()) return;
    setApiKey(key);
    // A reload rather than a state update: every page in the tree fetched and
    // failed already, and re-mounting them all is exactly what a reload does.
    location.reload();
  }
</script>

<div class="gate">
  <form novalidate class="card gate-card" onsubmit={submit}>
    <div class="card-header">
      <h1 class="card-title">
        <KeyRound size={18} />
        {t('ApiKeyRequired')}
      </h1>
    </div>

    {#if rejected}
      <ErrorBanner message={t('ApiKeyRejected')} />
    {/if}

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

    <button type="submit" class="btn btn-primary" disabled={!key.trim()}>{t('SaveKey')}</button>

    <p class="text-muted text-sm mt-3">
      {t('ApiKeyChooseOwn', { variable: 'ROUTARR_API_KEY' })}
    </p>
  </form>
</div>
