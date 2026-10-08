<script lang="ts">
  import { api } from '../api/client';
  import { formatCount } from '../api/format';
  import type { MetadataProvider } from '../api/types';
  import { createAsync } from '../lib/async.svelte';
  import { i18n, t } from '../lib/i18n.svelte';
  import { RefreshCw } from '../lib/icons';
  import { createOutcome } from '../lib/outcome.svelte';
  import { invalidateStatus, statusRevision } from '../lib/status.svelte';
  import ErrorBanner from './ErrorBanner.svelte';
  import OutcomeBanner from './OutcomeBanner.svelte';

  /**
   * One button per source a pass asks, to have every answer it gave read again
   * now, after the source corrected its data, rather than once the cache
   * lifetime has passed. The saved order, not the form's draft, says which
   * sources a pass asks: the list follows each save through the status
   * revision the save bumps.
   */
  const catalogue = createAsync((signal) => api.getMetadataProviders(signal), statusRevision);
  const asked = $derived(
    (catalogue.data?.order ?? [])
      .map((id) => catalogue.data?.providers.find((provider) => provider.id === id))
      .filter(
        (provider): provider is MetadataProvider =>
          provider !== undefined && provider.fetched && provider.configured,
      ),
  );

  const outcome = createOutcome();
  let running = $state<string | null>(null);

  async function refresh(provider: MetadataProvider) {
    running = provider.id;
    try {
      const report = await api.refreshSource(provider.id);
      invalidateStatus();
      const message = t('SourceRefreshed', {
        source: provider.display_name,
        enriched: formatCount(report.enriched, i18n.language),
        failed: formatCount(report.failed, i18n.language),
      });
      if (report.failed > 0) outcome.warn(message);
      else outcome.succeed(message);
    } catch (err) {
      outcome.fail(err);
    } finally {
      running = null;
    }
  }
</script>

<div class="card-header">
  <h2 class="card-title">{t('SourceRefreshTitle')}</h2>
</div>
<p class="text-muted text-md">{t('SourceRefreshHelp')}</p>
<ErrorBanner message={catalogue.error} onRetry={() => void catalogue.reload()} />
<OutcomeBanner {outcome} />
{#if asked.length > 0}
  <div class="source-list mt-2">
    {#each asked as provider (provider.id)}
      <div class="source-row">
        <span class="source-name">{provider.display_name}</span>
        <button
          type="button"
          class="btn btn-secondary btn-sm"
          disabled={running !== null}
          aria-label="{t('SourceRefresh')} – {provider.display_name}"
          onclick={() => void refresh(provider)}
        >
          <RefreshCw size={14} />
          {t('SourceRefresh')}
        </button>
      </div>
    {/each}
  </div>
{:else if catalogue.data}
  <p class="text-muted text-md mt-2">{t('SourceRefreshNone')}</p>
{/if}
