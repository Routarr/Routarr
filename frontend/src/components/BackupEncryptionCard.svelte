<script lang="ts">
  import CardTitle from './CardTitle.svelte';
  import { Lock } from '../lib/icons';
  import { api } from '../api/client';
  import { t } from '../lib/i18n.svelte';
  import type { Outcome } from '../lib/outcome.svelte';
  import { askConfirmation } from '../lib/confirm.svelte';
  import { withProof } from '../lib/proof.svelte';

  /**
   * The passphrase every archive is sealed with. Typed twice, since a typo
   * seals every archive with words nobody knows, and set, changed or removed
   * with the proof a key asks for: removing it opens every archive.
   */
  let { outcome, configured }: { outcome: Outcome; configured: boolean } = $props();

  /** The server's minimum, which `PUT /backups/passphrase` refuses below. */
  const SHORTEST = 12;

  /** What this card changed since the settings were read. */
  let changedTo = $state<boolean | null>(null);
  const encrypted = $derived(changedTo ?? configured);

  let next = $state('');
  let repeated = $state('');
  let saving = $state(false);

  const tooShort = $derived(next.length > 0 && [...next.trim()].length < SHORTEST);
  const mismatch = $derived(repeated.length > 0 && repeated !== next);
  const ready = $derived(next.trim().length > 0 && !tooShort && repeated === next);

  async function send(passphrase: string): Promise<boolean> {
    saving = true;
    try {
      return (await withProof((proof) => api.setBackupPassphrase(passphrase, proof))) !== null;
    } catch (cause) {
      outcome.fail(cause);
      return false;
    } finally {
      saving = false;
    }
  }

  async function save(event: SubmitEvent) {
    event.preventDefault();
    if (!ready || !(await send(next))) return;
    next = '';
    repeated = '';
    changedTo = true;
    outcome.succeed(t('PassphraseSaved'));
  }

  async function stop() {
    if (!(await askConfirmation(t('ConfirmStopEncrypting'), 'StopEncrypting'))) return;
    if (!(await send(''))) return;
    changedTo = false;
    outcome.succeed(t('EncryptionStopped'));
  }
</script>

<div class="card">
  <div class="card-header">
    <div>
      <!-- The state beside the name it qualifies: an archive left clear
           carries the master key, which is what the note warns of. Below
           it where both do not fit, rather than the name broken in two. -->
      <div class="flex flex-wrap items-center gap-2">
        <CardTitle card="BackupEncryption" class="flex items-center gap-2">
          <Lock size={18} aria-hidden="true" />
          {t('BackupEncryption')}
        </CardTitle>
        <span class="badge {encrypted ? 'badge-success' : 'badge-warning'}">
          {t(encrypted ? 'BackupEncryptionOn' : 'BackupEncryptionOff')}
        </span>
      </div>
      <p class="card-note">{t('BackupEncryptionHelp')}</p>
    </div>
  </div>

  <form novalidate onsubmit={save}>
    <div class="form-group">
      <label class="form-label" for="backup-passphrase-new">{t('NewPassphrase')}</label>
      <input
        id="backup-passphrase-new"
        type="password"
        class="form-input"
        autocomplete="new-password"
        aria-describedby="backup-passphrase-new-help"
        aria-invalid={tooShort}
        bind:value={next}
      />
      <p id="backup-passphrase-new-help" class="form-hint">
        {t('PasswordMinimum', { min: SHORTEST })}
        {t('BackupPassphraseKeep')}
      </p>
    </div>
    <div class="form-group">
      <label class="form-label" for="backup-passphrase-repeat">{t('RepeatPassphrase')}</label>
      <input
        id="backup-passphrase-repeat"
        type="password"
        class="form-input"
        autocomplete="new-password"
        aria-describedby={mismatch ? 'backup-passphrase-repeat-error' : undefined}
        aria-invalid={mismatch}
        bind:value={repeated}
      />
      {#if mismatch}
        <p id="backup-passphrase-repeat-error" class="field-error">{t('PassphrasesDiffer')}</p>
      {/if}
    </div>
    <div class="flex gap-2">
      <button type="submit" class="btn btn-primary" disabled={!ready || saving}>
        {t(encrypted ? 'ChangePassphrase' : 'EncryptArchives')}
      </button>
      {#if encrypted}
        <button type="button" class="btn btn-danger" disabled={saving} onclick={() => void stop()}>
          {t('StopEncrypting')}
        </button>
      {/if}
    </div>
  </form>
</div>
