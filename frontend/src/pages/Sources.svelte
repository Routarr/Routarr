<script lang="ts">
  import SettingsEditor from '../components/SettingsEditor.svelte';
  import SourceCredits from '../components/SourceCredits.svelte';
  import SourceRefresh from '../components/SourceRefresh.svelte';
  import { t } from '../lib/i18n.svelte';
</script>

<div>
  <div class="page-header">
    <div>
      <h1 class="page-title">{t('MetadataSources')}</h1>
      <p class="page-subtitle">{t('SourcesSubtitle')}</p>
    </div>
  </div>

  <SettingsEditor page="sources" />

  <div class="card mt-4">
    <SourceRefresh />
  </div>

  <div class="card mt-4">
    <SourceCredits />
  </div>
</div>
