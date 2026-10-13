<script lang="ts">
  import { Compass } from '../lib/icons';
  import { href, router } from '../lib/router.svelte';
  import { t } from '../lib/i18n.svelte';

  /**
   * A path this application does not serve. Rendered inside the shell, without a redirect, so
   * the mistyped address stays in view to correct and the menu is the way out.
   */
</script>

<div>
  <div class="page-header">
    <div>
      <h1 class="page-title">{t('NotFoundTitle')}</h1>
      <p class="page-subtitle">{t('NotFoundSubtitle', { path: router.path })}</p>
    </div>
    <div class="flex gap-2">
      <a class="btn btn-primary" href={href('/')}>{t('BackToDashboard')}</a>
    </div>
  </div>

  <div class="card">
    <div class="empty-state">
      <Compass size={32} />
      <p>{t('NotFoundHelp')}</p>
    </div>
  </div>
</div>
