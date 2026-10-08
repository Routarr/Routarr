<script lang="ts">
  import Modal from './Modal.svelte';
  import { t } from '../lib/i18n.svelte';
  import { proofRequest, settleProof } from '../lib/proof.svelte';

  /**
   * Asks for the password or the API key before a key is made or withdrawn,
   * and for the passphrase of a sealed archive before it is restored.
   * Mounted by `Layout` the first time a screen asks.
   */
  const WORDS = {
    password: {
      title: 'ProofPasswordTitle',
      help: 'ProofPasswordHelp',
      label: 'CurrentPassword',
      autocomplete: 'current-password',
    },
    key: {
      title: 'ProofKeyTitle',
      help: 'ProofKeyHelp',
      label: 'RoutarrApiKey',
      autocomplete: 'off',
    },
    passphrase: {
      title: 'ProofPassphraseTitle',
      help: 'ProofPassphraseHelp',
      label: 'Passphrase',
      autocomplete: 'off',
    },
  } as const;

  const open = $derived(proofRequest.request !== null);
  const words = $derived(WORDS[proofRequest.request?.asked ?? 'password']);
  const note = $derived(proofRequest.request?.note);
  let typed = $state('');

  function finish(value: string | null) {
    typed = '';
    settleProof(value);
  }

  function submit(event: SubmitEvent) {
    event.preventDefault();
    if (typed) finish(typed);
  }
</script>

{#if open}
  <Modal
    label={t(words.title)}
    onClose={() => finish(null)}
    maxWidth={460}
    initialFocus="proof-value"
  >
    <form novalidate onsubmit={submit}>
      <h2 class="card-title mb-3">{t(words.title)}</h2>
      {#if note}
        <p class="field-error mb-3" role="alert">{note}</p>
      {:else}
        <p class="text-muted text-base mb-3">{t(words.help)}</p>
      {/if}
      <div class="form-group">
        <label class="form-label" for="proof-value">{t(words.label)}</label>
        <input
          id="proof-value"
          type="password"
          class="form-input"
          autocomplete={words.autocomplete}
          bind:value={typed}
        />
      </div>
      <div class="dialog-actions">
        <button type="button" class="btn btn-secondary" onclick={() => finish(null)}>
          {t('Cancel')}
        </button>
        <button type="submit" class="btn btn-primary" disabled={!typed}>{t('Continue')}</button>
      </div>
    </form>
  </Modal>
{/if}
