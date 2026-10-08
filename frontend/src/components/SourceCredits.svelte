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
  /** The licence's own name, which every language writes as it is. */
  const LICENCE_NAME = 'CC BY-NC 4.0';
  /** OMDb's sentence cut around the licence's name, which becomes its link. */
  const omdbSentence = $derived(t('CreditOmdb').split(LICENCE_NAME));

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
      <!-- Running text, the site's link after the sentence, so a credit
           wraps as a sentence does rather than one piece per line. -->
      <li class="credit">
        {#if provider.id === 'tmdb'}
          <img class="credit-logo" src={tmdbLogo} alt={t('TmdbLogo')} />
        {/if}
        {#if provider.id === 'omdb' && omdbSentence.length === 2}
          {omdbSentence[0]}<a
            class="text-link"
            href={OMDB_LICENCE}
            target="_blank"
            rel="noopener noreferrer">{LICENCE_NAME}</a
          >{omdbSentence[1]}
        {:else}
          {t(CREDIT_KEY[provider.id] ?? '')}
        {/if}
        <a class="text-link" href={provider.website} target="_blank" rel="noopener noreferrer"
          >{host(provider.website ?? '')}</a
        >
      </li>
    {/each}
  </ul>
</section>
