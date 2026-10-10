<script lang="ts">
  import { Send } from '../lib/icons';
  import { api } from '../api/client';
  import { handFocus } from '../lib/focus';
  import { t } from '../lib/i18n.svelte';
  import type { Outcome } from '../lib/outcome.svelte';

  /**
   * A message sent to the saved notification address, in the saved format.
   *
   * Beside the settings form rather than in it: it sends what is saved, and a
   * change typed in the form is not saved until Save.
   */
  let { outcome }: { outcome: Outcome } = $props();

  let sending = $state(false);

  async function send() {
    const pressed = document.activeElement as HTMLElement | null;
    sending = true;
    try {
      await api.sendTestNotification();
      outcome.succeed(t('NotificationTestSent'));
    } catch (cause) {
      outcome.fail(cause);
    } finally {
      sending = false;
      void handFocus(pressed);
    }
  }
</script>

<div class="card">
  <div class="card-header">
    <div>
      <h2 class="card-title flex items-center gap-2">
        <Send size={18} aria-hidden="true" />
        {t('NotificationTest')}
      </h2>
      <p class="card-note">{t('NotificationTestHelp')}</p>
    </div>
  </div>
  <button type="button" class="btn btn-secondary" disabled={sending} onclick={() => void send()}>
    {t('SendTestNotification')}
  </button>
</div>
