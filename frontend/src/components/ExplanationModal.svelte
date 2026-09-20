<script lang="ts">
  import { api } from '../api/client';
  import { Lock, ShieldCheck } from '../lib/icons';
  import type { Explanation } from '../api/types';
  import { describeError } from '../lib/async.svelte';
  import { t } from '../lib/i18n.svelte';
  import ErrorBanner from './ErrorBanner.svelte';
  import Confidence from './Confidence.svelte';
  import EmptyState from './EmptyState.svelte';
  import Modal from './Modal.svelte';

  /** Condition-by-condition trace of why a media item lands where it does. */
  let { data, onClose }: { data: Explanation; onClose: () => void } = $props();

  /**
   * Pinning is one click because the panel already holds the whole answer.
   *
   * Everything a regression case needs — the item, the metadata that was
   * merged, the category the engine chose — was resolved to render this. Asking
   * the user to retype any of it is asking them not to bother.
   */
  let pinning = $state(false);
  let pinned = $state(false);
  let pinError = $state<string | null>(null);

  async function pin() {
    pinning = true;
    pinError = null;
    try {
      await api.pinRuleTest(
        t('PinnedCaseName', { title: data.media.title, category: data.target_category }),
        data.media.id,
        data.target_category,
      );
      pinned = true;
    } catch (err) {
      pinError = describeError(err);
    } finally {
      pinning = false;
    }
  }

  const OUTCOME_KEY: Record<string, string> = {
    winner: 'OutcomeWinner',
    excluded: 'OutcomeExcluded',
    matched_lower_priority: 'OutcomeLowerPriority',
    not_matched: 'OutcomeNotMatched',
  };
</script>

<Modal label={data.media.title} {onClose} maxWidth={780} maxHeight="88vh">
  <div class="modal-header">
    <h2 class="modal-title">{data.media.title}</h2>
    <div class="flex gap-2 items-center">
      <!-- Disabled once taken rather than hidden: a button that vanishes leaves
           the user unsure whether it worked. -->
      <button
        class="btn btn-secondary btn-sm"
        disabled={pinning || pinned}
        onclick={() => void pin()}
        title={t('PinAsRuleTestHint')}
      >
        <ShieldCheck size={14} />
        {pinned ? t('PinnedAsRuleTest') : t('PinAsRuleTest')}
      </button>
      <button
        class="btn btn-secondary btn-sm"
        onclick={onClose}
        aria-label={t('Dismiss')}
        title={t('Dismiss')}
      >
        ✕
      </button>
    </div>
  </div>

  <ErrorBanner message={pinError} onDismiss={() => (pinError = null)} />

  <div class="card card-inset">
    <div class="flex items-center justify-between">
      <div>
        <div class="text-muted text-md">{t('ProposedCategory')}</div>
        <div class="flex items-center gap-2 mt-2">
          <span class="badge badge-value">{data.target_category}</span>
          {#if data.override_category}
            <span class="badge badge-value muted">
              <Lock size={11} aria-hidden="true" />
              {t('ManualOverride')}
            </span>
          {/if}
          <span class="badge badge-value muted">{data.action}</span>
        </div>
      </div>
      <div class="w-120">
        <div class="text-muted text-md">{t('Confidence')}</div>
        <div class="mt-2"><Confidence value={data.confidence} meter /></div>
      </div>
    </div>

    <p class="text-muted text-md mt-4">
      <span class="mono">{data.media.current_root_folder ?? t('None')}</span>
      →
      <span class="mono">{data.target_root_folder ?? t('NoRootFolderMapped')}</span>
    </p>
  </div>

  {#if data.metadata}
    <div class="card">
      <div class="card-header">
        <h3 class="card-title">{t('MetadataTitle')}</h3>
      </div>
      <div class="flex flex-wrap gap-2 mt-2">
        {#each data.metadata.genres as genre (genre)}
          <span class="badge badge-info">{genre}</span>
        {/each}
      </div>
      <p class="text-muted text-md mt-2">
        {t('MetadataSummary', {
          language: data.metadata.original_language ?? t('None'),
          countries: data.metadata.origin_countries.join(', ') || t('None'),
          certification: data.metadata.certification ?? t('None'),
        })}
      </p>
      <div class="flex flex-wrap gap-2 mt-2">
        {#each data.metadata.keywords.slice(0, 15) as keyword (keyword)}
          <span class="badge badge-plain">{keyword}</span>
        {/each}
      </div>
      <!-- Which sources actually contributed, in priority order. With one
           source this is obvious; with several it is the only way to know
           whether a genre came from the library or from TMDb. -->
      <p class="text-muted text-sm mt-2">
        {t('MetadataSources')}:
        {#each data.metadata.sources as source, index (source)}
          {#if index > 0}<span class="dir-aware"> → </span>{/if}{source}
        {/each}
      </p>
    </div>
  {:else}
    <div class="banner banner-warning">{t('NoMetadataCached')}</div>
  {/if}

  <div class="card">
    <div class="card-header">
      <h3 class="card-title">{t('RuleEvaluation')}</h3>
    </div>
    {#if data.rule_traces.length === 0}
      <EmptyState>{t('NoRuleApplies')}</EmptyState>
    {:else}
      {#each data.rule_traces as trace (trace.rule_id)}
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
