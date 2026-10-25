<script lang="ts">
  import CardTitle from './CardTitle.svelte';
  import { api } from '../api/client';
  import { ArrowRight, Lock, RefreshCw, ShieldCheck } from '../lib/icons';
  import type { Explanation } from '../api/types';
  import { DECISION_ACTION_KEY, formatCount, formatDecimal, localName } from '../api/format';
  import { createAsync, describeError } from '../lib/async.svelte';
  import { cut, i18n, stored, t } from '../lib/i18n.svelte';
  import { createOutcome } from '../lib/outcome.svelte';
  import { invalidateStatus } from '../lib/status.svelte';
  import { askConfirmation } from '../lib/confirm.svelte';
  import { href } from '../lib/router.svelte';
  import { access } from '../lib/access.svelte';
  import { handFocus } from '../lib/focus';
  import NoValue from './NoValue.svelte';
  import ErrorBanner from './ErrorBanner.svelte';
  import OutcomeBanner from './OutcomeBanner.svelte';
  import Confidence from './Confidence.svelte';
  import EmptyState from './EmptyState.svelte';
  import Modal from './Modal.svelte';

  /** Condition-by-condition trace of why a media item lands where it does. */
  let {
    data,
    onClose,
    onChanged,
  }: {
    data: Explanation;
    onClose: () => void;
    /** Called once the panel changed where the title goes, so the list behind it reloads. */
    onChanged?: () => void;
  } = $props();

  /** The explanation read again once the sources were asked again, else the one given. */
  let refreshed = $state<Explanation | null>(null);
  const view = $derived(refreshed ?? data);

  // Cut where the sources go, so the arrows drawn between them mirror.
  const sourcesLine = $derived(cut('MetadataSourcesLine', ['sources']));
  /** How a source that finds a work by search found this title, by `identified_by`'s values. */
  const IDENTIFIED: Record<string, string> = {
    mapping: 'IdentifiedByMapping',
    shared: 'IdentifiedByShared',
    search: 'IdentifiedBySearch',
  };

  /**
   * Each source named and linked to its site, as the sources' terms ask of
   * whoever shows their data. Until the catalogue answers, its id stands in.
   */
  const catalogue = createAsync((signal) => api.getSources(signal));
  // The scales' names, which the condition catalogue holds.
  const conditions = createAsync((signal) => api.getConditionCatalog(signal));
  const providerOf = (id: string) => catalogue.data?.providers.find((p) => p.id === id);

  /** A sentence cut where its source goes, so the source is drawn as a link. */
  const around = (key: string, params: Record<string, string> = {}) => cut(key, ['source'], params);

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

  /** The verdict's reason in one sentence: the exception, the rule that won, or neither. */
  const decided = $derived.by(() => {
    if (view.override_category) {
      return t('ExplainDecidedByException', { category: view.override_category });
    }
    const winner = view.rule_traces.find((trace) => trace.outcome === 'winner');
    return winner
      ? t('ExplainDecidedByRule', { rule: winner.rule_name, priority: winner.priority })
      : t('ExplainDecidedByDefault');
  });

  /** The action whose own form is open under the footer, one at a time. */
  let opened = $state<'send' | 'pin' | null>(null);
  /** The action pointed at or focused, whose help shows under the footer. */
  let hinted = $state<'keep' | 'refresh' | null>(null);
  const point = (action: 'keep' | 'refresh') => ({
    onmouseenter: () => (hinted = action),
    onmouseleave: () => (hinted = null),
    onfocus: () => (hinted = action),
    onblur: () => (hinted = null),
  });

  let pinning = $state(false);
  let pinned = $state(false);
  let pinError = $state<string | null>(null);
  const categories = createAsync((signal) => api.getCategories(signal));
  // Null keeps the rules' category. Another pins a test that fails until a rule sends it there.
  let chosen = $state<string | null>(null);
  const expected = $derived(chosen ?? view.rules_category);
  const expectsOther = $derived(expected !== view.rules_category);

  /** Each scale's score, with its votes where it has them, in the reader's number form. */
  const scores = $derived.by(() => {
    const scales = conditions.data?.score_scales ?? [];
    // The catalogue's order, a scale it does not list last.
    const rank = (id: string) => {
      const at = scales.findIndex((known) => known.id === id);
      return at === -1 ? scales.length : at;
    };
    return Object.entries(view.metadata?.ratings ?? {})
      .sort(([a], [b]) => rank(a) - rank(b))
      .map(([scale, rating]) => {
        const known = scales.find((each) => each.id === scale);
        // A score out of 10 keeps its tenth, `8.0` as its site writes it.
        const value = formatDecimal(rating.value, i18n.language, known?.max === 10 ? 1 : 0);
        return {
          scale,
          name: known?.name ?? scale,
          shown:
            rating.votes === null
              ? value
              : t('WithNote', { text: value, note: formatCount(rating.votes, i18n.language) }),
        };
      });
  });

  /** What the Arr reports that a rule can read, each shown only when known. */
  const facts = $derived(
    [
      { label: t('ArrQualityProfile'), value: view.media.quality_profile },
      {
        label: t('ArrRuntime'),
        value:
          view.media.runtime === null ? null : t('RuntimeMinutes', { count: view.media.runtime }),
      },
      { label: t('ArrStudio'), value: view.media.studio },
      { label: t('ArrNetwork'), value: view.media.network },
      { label: t('ArrCollection'), value: view.media.collection },
    ].filter((fact): fact is { label: string; value: string } => fact.value !== null),
  );
  const categoryNames = $derived(
    categories.data?.map((category) => category.name) ?? [view.rules_category],
  );

  /** Stops following, not what the sources were asked, which runs on once the panel closes. */
  const leaving = new AbortController();
  $effect(() => () => leaving.abort());

  /**
   * Back to the pressed button once it is enabled again, else to the heading: Keep here and Pin
   * as test stay disabled once done.
   */
  const settleFocus = (pressed: Element | null) =>
    void handFocus(pressed as HTMLElement | null, 'explain-title');

  async function pin() {
    const pressed = document.activeElement;
    pinning = true;
    pinError = null;
    try {
      const name = stored(expectsOther ? 'PinnedCaseNameExpected' : 'PinnedCaseName', {
        title: view.media.title,
        category: expected,
      });
      await api.pinRuleTest(name, view.media.id, expected);
      pinned = true;
      invalidateStatus();
    } catch (err) {
      pinError = describeError(err);
    } finally {
      pinning = false;
      settleFocus(pressed);
    }
  }

  // Keeping pins the title to its current folder's category, which the server reads again.
  let keeping = $state(false);
  const kept = $derived(
    view.current_category !== null && view.override_category === view.current_category,
  );

  async function keep() {
    const category = view.current_category;
    if (category === null) return;
    const replaced = view.override_category;
    if (
      replaced !== null &&
      !(await askConfirmation(t('KeepReplacesException', { category: replaced }), 'KeepHere'))
    ) {
      return;
    }
    const pressed = document.activeElement;
    keeping = true;
    try {
      await api.keepTitle(view.media.id);
      refreshed = await api.explainTitle(view.media.id, leaving.signal);
      invalidateStatus();
      onChanged?.();
      outcome.succeed(t('KeptHereOutcome', { category }));
    } catch (err) {
      if (!leaving.signal.aborted) outcome.fail(err);
    } finally {
      keeping = false;
      settleFocus(pressed);
    }
  }

  let sending = $state(false);
  let sendError = $state<string | null>(null);
  // Null shows the category the title goes to now, so the choice made reads as a change.
  let sendTo = $state<string | null>(null);
  const sendCategory = $derived(sendTo ?? view.target_category);
  let sendReason = $state('');

  async function send() {
    if (sending) return;
    const pressed = document.activeElement;
    const category = sendCategory;
    sending = true;
    sendError = null;
    try {
      await api.createException({
        media_id: view.media.id,
        target_category: category,
        reason: sendReason.trim() || null,
      });
      refreshed = await api.explainTitle(view.media.id, leaving.signal);
      opened = null;
      sendTo = null;
      sendReason = '';
      invalidateStatus();
      onChanged?.();
      outcome.succeed(t('OverrideCreated', { title: view.media.title, category }));
    } catch (err) {
      if (!leaving.signal.aborted) sendError = describeError(err);
    } finally {
      sending = false;
      // Back to the button that opened the form once it folds, else to the one pressed.
      void handFocus(opened === 'send' ? (pressed as HTMLElement | null) : 'explain-send');
    }
  }

  const outcome = createOutcome();
  let refreshing = $state(false);

  /**
   * Asks every source again about this title, its AniList and MyAnimeList
   * matches forgotten, then reads the explanation again from what they said.
   */
  async function refresh() {
    const pressed = document.activeElement;
    refreshing = true;
    try {
      const { answered } = await api.refreshTitleMetadata(view.media.id, {
        signal: leaving.signal,
      });
      refreshed = await api.explainTitle(view.media.id, leaving.signal);
      invalidateStatus();
      if (answered > 0) {
        outcome.succeed(t('TitleRefreshed', { count: formatCount(answered, i18n.language) }));
      } else {
        outcome.warn(t('TitleRefreshedByNone'));
      }
    } catch (err) {
      if (!leaving.signal.aborted) outcome.fail(err);
    } finally {
      refreshing = false;
      settleFocus(pressed);
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
     its first button would take a second Enter as a press nobody asked for. -->
<Modal
  title={view.media.title}
  titleId="explain-title"
  {onClose}
  maxWidth={780}
  maxHeight="88vh"
  initialFocus="explain-title"
>
  <ErrorBanner message={categories.error} onRetry={() => void categories.reload()} />
  <ErrorBanner message={catalogue.error} onRetry={() => void catalogue.reload()} />
  <ErrorBanner message={conditions.error} onRetry={() => void conditions.reload()} />
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
    <p class="text-md mt-2">{decided}</p>
    {#if facts.length > 0}
      <dl class="fact-list">
        {#each facts as fact (fact.label)}
          <div>
            <dt>{fact.label}</dt>
            <dd>{fact.value}</dd>
          </div>
        {/each}
      </dl>
    {/if}
    {#if !view.instance_enabled}
      <p class="text-warning text-md mt-2">{t('ExplainInstanceOff')}</p>
    {/if}
  </div>

  <div class="card">
    <div class="card-header">
      <CardTitle card="RuleEvaluation" level={3}>{t('RuleEvaluation')}</CardTitle>
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
                <!-- The glyph and its colour for the eye, the word for a screen reader. -->
                <span
                  class={condition.matched ? 'explain-match' : 'explain-miss'}
                  aria-hidden="true"
                >
                  {condition.matched ? '✓' : '✗'}
                </span>
                <span class="visually-hidden">
                  {t(condition.matched ? 'ConditionHeld' : 'ConditionNotHeld')}
                </span>
                <span class="flex-1">{condition.expected}</span>
                <span class="explain-observed">
                  {condition.observed || t('Unknown')}
                  {#if condition.source}
                    <span class="text-muted text-xs ms-1">
                      {t('MetadataFromSource', {
                        source: providerOf(condition.source)?.display_name ?? condition.source,
                      })}
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
  {#if view.metadata}
    <div class="card">
      <div class="card-header">
        <CardTitle card="MetadataTitle" level={3}>{t('MetadataTitle')}</CardTitle>
      </div>
      <div class="flex flex-wrap gap-2 mt-2">
        {#each view.metadata.genres as genre (genre)}
          <span class="badge badge-info">{genre}</span>
        {/each}
      </div>
      {#if scores.length > 0}
        <dl class="fact-list" aria-label={t('FacetRatings')}>
          {#each scores as score (score.scale)}
            <div>
              <dt>{score.name}</dt>
              <dd>{score.shown}</dd>
            </div>
          {/each}
        </dl>
      {/if}
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
      <!-- Each value's source, in priority order: with several, nothing else tells whether a
           genre came from the library or from TMDB. -->
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
      <!-- A search can name another work of the same title: said, so a wrong match is seen. -->
      {#each Object.entries(view.identified_by) as [source, how] (source)}
        <p class="text-muted text-sm">
          {t(IDENTIFIED[how] ?? 'IdentifiedBySearch', {
            source: providerOf(source)?.display_name ?? source,
          })}
        </p>
      {/each}
    </div>
  {:else}
    <div class="banner banner-warning">{t('NoMetadataCached')}</div>
  {/if}

  {#if !access.readOnly}
    <!-- After the answer, and sticky: a long trace keeps the actions in reach. -->
    <div class="explain-footer">
      <div class="flex flex-wrap gap-2">
        <button
          class="btn btn-secondary btn-sm"
          disabled={keeping || kept || view.current_category === null}
          onclick={() => void keep()}
          aria-describedby="explain-keep-hint"
          {...point('keep')}
        >
          <Lock size={14} />
          {t('KeepHere')}
        </button>
        <button
          id="explain-send"
          class="btn btn-secondary btn-sm"
          aria-expanded={opened === 'send'}
          aria-controls="explain-send-form"
          onclick={() => (opened = opened === 'send' ? null : 'send')}
        >
          <ArrowRight size={14} class="dir-aware" />
          {t('SendToCategory')}
        </button>
        <button
          id="explain-pin"
          class="btn btn-secondary btn-sm"
          aria-expanded={opened === 'pin'}
          aria-controls="explain-pin-form"
          onclick={() => (opened = opened === 'pin' ? null : 'pin')}
        >
          <ShieldCheck size={14} />
          {t('PinAsRuleTestOpen')}
        </button>
        <button
          class="btn btn-secondary btn-sm"
          disabled={refreshing}
          onclick={() => void refresh()}
          aria-describedby="explain-refresh-hint"
          {...point('refresh')}
        >
          <RefreshCw size={14} />
          {t('RefreshTitleMetadata')}
        </button>
      </div>

      <!-- A state stays in sight, a help only for the action at hand. Hidden, each still
           describes its button. -->
      <p
        id="explain-keep-hint"
        class="form-hint"
        hidden={view.current_category !== null && !kept && hinted !== 'keep'}
      >
        {#if view.current_category === null}
          {t('KeepHereUnmapped')}
          <a class="text-link" href={href('/categories')}>{t('GuideCategoriesAction')}</a>
        {:else if kept}
          {t('KeptHere', { category: view.current_category })}
        {:else}
          {t('KeepHereHint', { category: view.current_category })}
        {/if}
      </p>
      <p id="explain-refresh-hint" class="form-hint" hidden={hinted !== 'refresh'}>
        {t('RefreshTitleMetadataHint')}
      </p>

      <div id="explain-send-form" class="explain-form" hidden={opened !== 'send'}>
        <ErrorBanner message={sendError} onDismiss={() => (sendError = null)} />
        <div class="form-row">
          <div class="form-group">
            <label class="form-label" for="explain-send-category">{t('ForcedCategory')}</label>
            <select
              id="explain-send-category"
              class="form-select"
              value={sendCategory}
              onchange={(event) => (sendTo = event.currentTarget.value)}
            >
              {#each categoryNames as name (name)}
                <option value={name}>{name}</option>
              {/each}
            </select>
          </div>
          <div class="form-group">
            <label class="form-label" for="explain-send-reason">
              {t('WithNote', { text: t('Reason'), note: t('Optional') })}
            </label>
            <input
              id="explain-send-reason"
              class="form-input"
              placeholder={t('ReasonPlaceholder')}
              bind:value={sendReason}
            />
          </div>
        </div>
        <button class="btn btn-primary btn-sm" disabled={sending} onclick={() => void send()}>
          {sending ? t('Saving') : t('CreateOverride')}
        </button>
      </div>

      <div id="explain-pin-form" class="explain-form" hidden={opened !== 'pin'}>
        <ErrorBanner message={pinError} onDismiss={() => (pinError = null)} />
        <div class="form-group">
          <label class="form-label" for="explain-pin-category">{t('PinExpectedCategory')}</label>
          <select
            id="explain-pin-category"
            class="form-select"
            value={expected}
            disabled={pinned}
            onchange={(event) => (chosen = event.currentTarget.value)}
          >
            {#each categoryNames as name (name)}
              <option value={name}>{name}</option>
            {/each}
          </select>
        </div>
        <!-- Disabled once taken rather than hidden: a button that vanishes leaves
             the user unsure whether it worked. -->
        <button
          class="btn btn-primary btn-sm"
          disabled={pinning || pinned}
          onclick={() => void pin()}
          aria-describedby="explain-pin-hint{view.override_category ? ' explain-pin-rules' : ''}"
        >
          <ShieldCheck size={14} />
          {pinned ? t('PinnedAsRuleTest') : t('PinAsRuleTest')}
        </button>
        <!-- In sight beside the button: while a test fails, automatic application waits. -->
        <p id="explain-pin-hint" class="form-hint">
          {expectsOther
            ? t('PinAsRuleTestExpectedHint', { category: expected })
            : t('PinAsRuleTestHint')}
        </p>
        {#if view.override_category && !expectsOther}
          <p id="explain-pin-rules" class="form-hint">
            {t('PinRulesUnderException', { category: view.rules_category })}
          </p>
        {/if}
      </div>
    </div>
  {/if}
</Modal>
