<script lang="ts">
  import { api } from '../api/client';
  import { Lock, RefreshCw, ShieldCheck } from '../lib/icons';
  import type { Explanation } from '../api/types';
  import { DECISION_ACTION_KEY, formatCount, localName } from '../api/format';
  import { createAsync, describeError } from '../lib/async.svelte';
  import { i18n, t } from '../lib/i18n.svelte';
  import { createOutcome } from '../lib/outcome.svelte';
  import { invalidateStatus } from '../lib/status.svelte';
  import NoValue from './NoValue.svelte';
  import ErrorBanner from './ErrorBanner.svelte';
  import OutcomeBanner from './OutcomeBanner.svelte';
  import Confidence from './Confidence.svelte';
  import EmptyState from './EmptyState.svelte';
  import Modal from './Modal.svelte';

  /** Condition-by-condition trace of why a media item lands where it does. */
  let { data, onClose }: { data: Explanation; onClose: () => void } = $props();

  /** The explanation read again once the sources were asked again, else the one given. */
  let refreshed = $state<Explanation | null>(null);
  const view = $derived(refreshed ?? data);

  // The sources line holds arrows that mirror between its values, so the
  // summary is cut where the values go and drawn around them.
  const sourcesLine = $derived(t('MetadataSourcesLine', { sources: '\u0000' }).split('\u0000'));

  /**
   * Each source named and linked to its site, as the sources' terms ask of
   * whoever shows their data. Until the catalogue answers, its id stands in.
   */
  const catalogue = createAsync((signal) => api.getMetadataProviders(signal));
  const providerOf = (id: string) => catalogue.data?.providers.find((p) => p.id === id);

  /** A sentence cut where its source goes, so the source is drawn as a link. */
  const around = (key: string, params: Record<string, string> = {}) =>
    t(key, { ...params, source: '\u0000' }).split('\u0000');

  /**
   * The dictionary key naming a title's status, in the words Radarr and Sonarr
   * write it, which every metadata source's status is read into.
   */
  const ARR_STATUS_KEY: Record<string, string> = {
    tba: 'ArrStatusTba',
    announced: 'ArrStatusAnnounced',
    inCinemas: 'ArrStatusInCinemas',
    released: 'ArrStatusReleased',
    upcoming: 'ArrStatusUpcoming',
    continuing: 'ArrStatusContinuing',
    ended: 'ArrStatusEnded',
    deleted: 'ArrStatusDeleted',
  };

  const status = $derived(view.metadata?.status ?? null);
  const statusLine = $derived(
    status
      ? around('MetadataStatusLine', {
          status: ARR_STATUS_KEY[status] ? t(ARR_STATUS_KEY[status]) : status,
        })
      : [],
  );
  const synopsisLine = $derived(around('SynopsisFrom'));

  /**
   * Pinning is one click because the panel already holds the whole answer.
   *
   * Everything a regression case needs (the item, the metadata that was
   * merged, the category the engine chose) is already resolved to render this.
   * Asking the user to retype any of it is asking them not to bother.
   */
  let pinning = $state(false);
  let pinned = $state(false);
  let pinError = $state<string | null>(null);

  async function pin() {
    pinning = true;
    pinError = null;
    try {
      await api.pinRuleTest(
        t('PinnedCaseName', { title: view.media.title, category: view.target_category }),
        view.media.id,
        view.target_category,
      );
      pinned = true;
    } catch (err) {
      pinError = describeError(err);
    } finally {
      pinning = false;
    }
  }

  /**
   * Asks every source again about this title, its AniList and MyAnimeList
   * matches forgotten, then reads the explanation again from what they said.
   */
  const outcome = createOutcome();
  let refreshing = $state(false);

  async function refresh() {
    refreshing = true;
    try {
      const { answered } = await api.refreshTitleMetadata(view.media.id);
      refreshed = await api.explainMedia(view.media.id);
      invalidateStatus();
      if (answered > 0) {
        outcome.succeed(t('TitleRefreshed', { count: formatCount(answered, i18n.language) }));
      } else {
        outcome.warn(t('TitleRefreshedByNone'));
      }
    } catch (err) {
      outcome.fail(err);
    } finally {
      refreshing = false;
    }
  }

  /** A language or a country as the rule editor names it, the code beside the name. */
  const nameOf = (type: 'language' | 'region', code: string) =>
    localName(type, code, i18n.language) ?? code;

  const OUTCOME_KEY: Record<string, string> = {
    winner: 'OutcomeWinner',
    excluded: 'OutcomeExcluded',
    matched_lower_priority: 'OutcomeLowerPriority',
    not_matched: 'OutcomeNotMatched',
  };
</script>

{#snippet named(id: string)}
  {@const provider = providerOf(id)}
  {#if provider?.website}
    <a class="text-link" href={provider.website} target="_blank" rel="noopener noreferrer"
      >{provider.display_name}</a
    >
  {:else}
    {provider?.display_name ?? id}
  {/if}
{/snippet}

<!-- Opened on its heading: opened from the quick search by Enter, a panel on
     its first button would take a second Enter as a pin nobody asked for. -->
<Modal
  label={view.media.title}
  {onClose}
  maxWidth={780}
  maxHeight="88vh"
  initialFocus="explain-title"
>
  <div class="modal-header">
    <h2 id="explain-title" class="modal-title" tabindex="-1">{view.media.title}</h2>
    <button
      class="btn btn-secondary btn-sm"
      onclick={onClose}
      aria-label={t('Dismiss')}
      title={t('Dismiss')}
    >
      ✕
    </button>
  </div>

  <div class="explain-actions">
    <div class="explain-action">
      <button
        class="btn btn-secondary btn-sm"
        disabled={refreshing}
        onclick={() => void refresh()}
        aria-describedby="explain-refresh-hint"
      >
        <RefreshCw size={14} />
        {t('RefreshTitleMetadata')}
      </button>
      <p id="explain-refresh-hint" class="form-hint">{t('RefreshTitleMetadataHint')}</p>
    </div>
    <div class="explain-action">
      <!-- Disabled once taken rather than hidden: a button that vanishes leaves
           the user unsure whether it worked. -->
      <button
        class="btn btn-secondary btn-sm"
        disabled={pinning || pinned}
        onclick={() => void pin()}
        aria-describedby="explain-pin-hint"
      >
        <ShieldCheck size={14} />
        {pinned ? t('PinnedAsRuleTest') : t('PinAsRuleTest')}
      </button>
      <p id="explain-pin-hint" class="form-hint">{t('PinAsRuleTestHint')}</p>
    </div>
  </div>

  <ErrorBanner message={pinError} onDismiss={() => (pinError = null)} />
  <ErrorBanner message={catalogue.error} onRetry={() => void catalogue.reload()} />
  <OutcomeBanner {outcome} />

  <div class="card card-inset">
    <div class="flex items-center justify-between">
      <div>
        <div class="text-muted text-md">{t('ProposedCategory')}</div>
        <div class="flex items-center gap-2 mt-2">
          <span class="badge badge-value">{view.target_category}</span>
          {#if view.override_category}
            <span class="badge badge-value muted">
              <Lock size={11} aria-hidden="true" />
              {t('ManualOverride')}
            </span>
          {/if}
          <span class="badge badge-value muted">{t(DECISION_ACTION_KEY[view.action])}</span>
        </div>
      </div>
      <div class="w-120">
        <div class="text-muted text-md">{t('Confidence')}</div>
        <div class="mt-2"><Confidence value={view.confidence} meter /></div>
      </div>
    </div>

    <p class="text-muted text-md mt-4">
      {#if view.media.current_root_folder}
        <span class="mono">{view.media.current_root_folder}</span>
      {:else}
        <NoValue />
      {/if}
      <span class="dir-aware">→</span>
      <span class="mono">{view.target_root_folder ?? t('NoRootFolderMapped')}</span>
    </p>
    {#if !view.instance_enabled}
      <p class="text-warning text-md mt-2">{t('ExplainInstanceOff')}</p>
    {/if}
  </div>

  {#if view.metadata}
    <div class="card">
      <div class="card-header">
        <h3 class="card-title">{t('MetadataTitle')}</h3>
      </div>
      <div class="flex flex-wrap gap-2 mt-2">
        {#each view.metadata.genres as genre (genre)}
          <span class="badge badge-info">{genre}</span>
        {/each}
      </div>
      <p class="text-muted text-md mt-2">
        {t('MetadataSummary', {
          language: view.metadata.original_language
            ? nameOf('language', view.metadata.original_language)
            : t('NoneSpoken'),
          countries:
            view.metadata.origin_countries
              .map((country) => nameOf('region', country))
              .join(t('ListSeparator')) || t('NoneSpoken'),
          certification: view.metadata.certification ?? t('NoneSpoken'),
        })}
      </p>
      <div class="flex flex-wrap gap-2 mt-2">
        {#each view.metadata.keywords.slice(0, 15) as keyword (keyword)}
          <span class="badge badge-plain">{keyword}</span>
        {/each}
      </div>
      <!-- Which sources actually contributed, in priority order. With one
           source this is obvious. With several it is the only way to know
           whether a genre came from the library or from TMDB. -->
      {#if status}
        <p class="text-md mt-2">
          {statusLine[0]}{@render named(view.metadata.field_sources.status ?? '')}{statusLine[1]}
        </p>
      {/if}
      {#if view.metadata.overview}
        <p class="text-md mt-2">{view.metadata.overview}</p>
        <p class="text-muted text-sm mt-1">
          {synopsisLine[0]}{@render named(
            view.metadata.field_sources.overview ?? '',
          )}{synopsisLine[1]}
        </p>
      {/if}
      <p class="text-muted text-sm mt-2">
        {sourcesLine[0]}{#each view.metadata.sources as source, index (source)}
          {#if index > 0}<span class="dir-aware"> → </span>{/if}{@render named(source)}
        {/each}{sourcesLine[1]}
      </p>
    </div>
  {:else}
    <div class="banner banner-warning">{t('NoMetadataCached')}</div>
  {/if}

  <div class="card">
    <div class="card-header">
      <h3 class="card-title">{t('RuleEvaluation')}</h3>
    </div>
    {#if view.rule_traces.length === 0}
      <EmptyState>{t('NoRuleApplies')}</EmptyState>
    {:else}
      {#each view.rule_traces as trace (trace.rule_id)}
        <div class="mt-4">
          <div class="flex items-center gap-2">
            <strong>{trace.rule_name}</strong>
            <span class="num">#{trace.priority}</span>
            <span class="badge badge-value">{trace.category}</span>
            <span
              class="badge {trace.outcome === 'winner'
                ? 'badge-success'
                : trace.outcome === 'excluded'
                  ? 'badge-danger'
                  : 'badge-warning'}"
            >
              {t(OUTCOME_KEY[trace.outcome] ?? 'OutcomeNotMatched')}
            </span>
          </div>

          {#if trace.excluded_by}
            <p class="text-danger text-sm mt-2">
              ⛔ {t('VetoedByExclusion', { reason: trace.excluded_by })}
            </p>
          {/if}

          <div class="mt-2">
            {#each trace.conditions as condition, index (index)}
              <div class="explain-row">
                <span class={condition.matched ? 'explain-match' : 'explain-miss'}>
                  {condition.matched ? '✓' : '✗'}
                </span>
                <span class="flex-1">{condition.expected}</span>
                <span class="explain-observed">
                  {condition.observed || t('Unknown')}
                  {#if condition.source}
                    <span class="text-muted text-xs ms-1">
                      {t('MetadataFromSource', { source: condition.source })}
                    </span>
                  {/if}
                </span>
              </div>
            {/each}
          </div>
        </div>
      {/each}
    {/if}
  </div>
</Modal>
