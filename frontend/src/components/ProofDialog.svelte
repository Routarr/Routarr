<script lang="ts">
  import Modal from './Modal.svelte';
  import { t } from '../lib/i18n.svelte';
  import { proofRequest, settleProof } from '../lib/proof.svelte';

  /**
   * Asks for the password or the API key before a key is made or withdrawn.
   * Mounted once, by `Layout`.
   */
  const open = $derived(proofRequest.request !== null);
  const asked = $derived(proofRequest.request?.asked ?? 'password');
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
    label={t(asked === 'password' ? 'ProofPasswordTitle' : 'ProofKeyTitle')}
    onClose={() => finish(null)}
    maxWidth={460}
    initialFocus="proof-value"
  >
    <form novalidate onsubmit={submit}>
      <h2 class="card-title mb-3">
        {t(asked === 'password' ? 'ProofPasswordTitle' : 'ProofKeyTitle')}
      </h2>
      <p class="text-muted text-base mb-3">
        {t(asked === 'password' ? 'ProofPasswordHelp' : 'ProofKeyHelp')}
      </p>
      <div class="form-row">
        <label class="form-label" for="proof-value">
          {t(asked === 'password' ? 'CurrentPassword' : 'RoutarrApiKey')}
        </label>
        <input
          id="proof-value"
          type="password"
          class="form-input"
          autocomplete={asked === 'password' ? 'current-password' : 'off'}
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
