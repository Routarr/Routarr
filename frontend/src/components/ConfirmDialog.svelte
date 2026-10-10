<script lang="ts">
  import Modal from './Modal.svelte';
  import { t } from '../lib/i18n.svelte';
  import { confirmation, settle } from '../lib/confirm.svelte';

  /**
   * Renders whatever `ask()` is waiting on. Mounted once, by `Layout`.
   *
   * The values are read through `$derived` rather than a `{@const}` in the
   * markup: a `{@const}` is reactive, so a handler that settles the request
   * before reading it back reads `null` and the action never happens.
   */
  const open = $derived(confirmation.request !== null);
  const message = $derived(confirmation.request?.message ?? '');
  const choices = $derived(confirmation.request?.choices ?? []);
</script>

{#if open}
  <!-- Named by what it would do and described by the question, which a
       screen reader then reads once, as an alert dialog's description. -->
  <Modal
    label={choices[0] ? t(choices[0].label) : message}
    onClose={() => settle(null)}
    maxWidth={520}
    alert
    describedBy="confirm-question"
  >
    <p id="confirm-question" class="pre-line">{message}</p>
    <div class="dialog-actions">
      <button class="btn btn-secondary" onclick={() => settle(null)}>{t('Cancel')}</button>
      <div class="flex flex-wrap gap-2">
        {#each choices as choice (choice.value)}
          <button
            class="btn {choice.danger ? 'btn-danger' : 'btn-primary'}"
            onclick={() => settle(choice.value)}
          >
            {t(choice.label)}
          </button>
        {/each}
      </div>
    </div>
  </Modal>
{/if}
