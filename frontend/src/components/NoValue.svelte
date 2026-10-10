<script lang="ts">
  import { t } from '../lib/i18n.svelte';

  /**
   * A cell with nothing in it: a dash for the eye, and a word for a screen
   * reader, which reads a lone dash as "dash" or not at all.
   */
</script>

<span aria-hidden="true">{t('None')}</span><span class="visually-hidden">{t('NoneSpoken')}</span>
