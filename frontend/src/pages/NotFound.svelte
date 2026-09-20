<script lang="ts">
  import { Compass } from '../lib/icons';
  import { href, router } from '../lib/router.svelte';
  import { t } from '../lib/i18n.svelte';

  /**
   * A path this application does not serve.
   *
   * Rendered rather than redirected, and the URL is left alone. Every link here
   * is written by the application, so a miss is a typed address or an old
   * bookmark — and both are worth correcting, which is impossible once the
   * address has been rewritten. Showing the dashboard instead, as this did,
   * left somebody looking at a screen they had not asked for with nothing
   * saying so and no navigation entry marked active.
   *
   * Inside the shell on purpose: the menu is the way out, and a page that
   * replaced it would make a mistyped character look like a broken install.
   */
</script>

<div>
  <div class="page-header">
    <div>
      <h1 class="page-title">{t('NotFoundTitle')}</h1>
      <p class="page-subtitle">{t('NotFoundSubtitle', { path: router.path })}</p>
    </div>
  </div>

  <div class="empty-state">
    <Compass size={32} />
    <p>{t('NotFoundHelp')}</p>
    <a class="btn btn-primary" href={href('/')}>{t('BackToDashboard')}</a>
  </div>
</div>
