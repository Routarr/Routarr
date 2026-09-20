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
  <!-- The question is the dialog's accessible name: it is the whole content,
       and a separate title would say less than the sentence already does. -->
  <Modal label={message} onClose={() => settle(null)} maxWidth={520}>
    <p class="pre-line">{message}</p>
    <div class="flex gap-2 mt-4">
      <button class="btn btn-secondary" onclick={() => settle(null)}>{t('Cancel')}</button>
      {#each choices as choice (choice.value)}
        <button
          class="btn {choice.danger ? 'btn-danger' : 'btn-primary'}"
          onclick={() => settle(choice.value)}
        >
          {t(choice.label)}
        </button>
      {/each}
    </div>
  </Modal>
{/if}
