<script lang="ts">
  import { api } from '../api/client';
  import { createAsync } from '../lib/async.svelte';
  import { t } from '../lib/i18n.svelte';
  import ErrorBanner from './ErrorBanner.svelte';
  import tmdbLogo from '../assets/tmdb.svg';

  /**
   * What each metadata source's terms ask for, wherever its data is used:
   * TMDB a notice and its logo, TheTVDB a sentence and a link, OMDb its
   * licence. Every source is credited, enabled or not, so the credit is never
   * missing the day one is turned on.
   */
  const CREDIT_KEY: Record<string, string> = {
    tmdb: 'CreditTmdb',
    tvdb: 'CreditTvdb',
    omdb: 'CreditOmdb',
    anilist: 'CreditAnilist',
    jikan: 'CreditJikan',
  };
  const OMDB_LICENCE = 'https://creativecommons.org/licenses/by-nc/4.0/';

  const catalogue = createAsync((signal) => api.getMetadataProviders(signal));
  const credited = $derived(
    (catalogue.data?.providers ?? []).filter(
      (provider) => provider.id in CREDIT_KEY && provider.website,
    ),
  );

  const host = (site: string) => new URL(site).host;
</script>

<section aria-labelledby="source-credits-title">
  <h2 id="source-credits-title" class="card-title">{t('SourceCreditsTitle')}</h2>
  <ErrorBanner message={catalogue.error} onRetry={() => void catalogue.reload()} />
  <ul class="credits">
    {#each credited as provider (provider.id)}
      <li class="credit">
        {#if provider.id === 'tmdb'}
          <img class="credit-logo" src={tmdbLogo} alt={t('TmdbLogo')} />
        {/if}
        <span>{t(CREDIT_KEY[provider.id] ?? '')}</span>
        <a href={provider.website} target="_blank" rel="noopener noreferrer">
          {host(provider.website ?? '')}
        </a>
        {#if provider.id === 'omdb'}
          <a href={OMDB_LICENCE} target="_blank" rel="noopener noreferrer">CC BY-NC 4.0</a>
        {/if}
      </li>
    {/each}
  </ul>
</section>
