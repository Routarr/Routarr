<script lang="ts">
  import SettingsEditor from '../components/SettingsEditor.svelte';
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
</div>
