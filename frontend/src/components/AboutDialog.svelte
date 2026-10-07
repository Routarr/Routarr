<script lang="ts">
  import { t } from '../lib/i18n.svelte';
  import Modal from './Modal.svelte';
  import SourceCredits from './SourceCredits.svelte';

  /** What Routarr is: its version, its licence, and its sources' credits. */
  let { version, onClose }: { version: string; onClose: () => void } = $props();

  const REPOSITORY = 'https://github.com/Routarr/Routarr';
</script>

<Modal label={t('AboutRoutarr')} {onClose} maxWidth={560} closeOnBackdrop>
  <div class="modal-header">
    <h2 class="modal-title">{t('AboutRoutarr')}</h2>
    <button
      class="btn btn-secondary btn-sm"
      onclick={onClose}
      aria-label={t('Dismiss')}
      title={t('Dismiss')}
    >
      ✕
    </button>
  </div>
  <p class="text-md">{t('AboutVersion', { version })}</p>
  <p class="text-muted text-md mt-2">
    {t('AboutLicence')}
    <a class="text-link" href={REPOSITORY} target="_blank" rel="noopener noreferrer"
      >github.com/Routarr/Routarr</a
    >
  </p>
  <div class="mt-4">
    <SourceCredits />
  </div>
</Modal>
