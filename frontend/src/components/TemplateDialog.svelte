<script lang="ts">
  import { untrack } from 'svelte';
  import Modal from './Modal.svelte';
  import CardTitle from './CardTitle.svelte';
  import ValuePicker from './ValuePicker.svelte';
  import TableRegion from './TableRegion.svelte';
  import Stat from './Stat.svelte';
  import ErrorBanner from './ErrorBanner.svelte';
  import { api } from '../api/client';
  import type {
    Category,
    DecisionAction,
    Facet,
    TemplateAdded,
    TemplateRequest,
    TemplateSummary,
  } from '../api/types';
  import { createAsync, describeError } from '../lib/async.svelte';
  import { askConfirmation } from '../lib/confirm.svelte';
  import { href } from '../lib/router.svelte';
  import { formatCount } from '../api/format';
  import { Info } from '../lib/icons';
  import { i18n, t } from '../lib/i18n.svelte';

  /** What adding one template asks, and what it would do, before it is added. */
  let {
    template,
    categories,
    languageOptions,
    onClose,
    onAdded,
  }: {
    template: TemplateSummary;
    categories: Category[];
    languageOptions: Facet[];
    onClose: () => void;
    onAdded: (added: TemplateAdded) => void;
  } = $props();

  // Its own copy of what the gallery offered: the earlier answers, or the suggested ones.
  const offered = untrack(() => template);
  let category = $state(offered.category ?? '');
  let languages = $state([...offered.languages]);
  let requesters = $state(
    offered.requesters.map((each) => ({ ...each, chosen: each.added, target: each.category })),
  );
  let failure = $state<string | null>(null);
  let adding = $state(false);

  const replacing = $derived(template.added_version !== null);
  const byRequester = $derived(template.parameter === 'requesters');
  const firstField = $derived(
    !byRequester
      ? 'template-category'
      : requesters.length > 0
        ? 'template-requester-check-0'
        : 'template-requesters-none',
  );
  const chosen = $derived(requesters.filter((each) => each.chosen));
  const request = $derived<TemplateRequest>({
    ...(byRequester ? {} : { category: category.trim() }),
    ...(template.parameter === 'languages' ? { languages } : {}),
    ...(byRequester
      ? { requesters: chosen.map((each) => ({ tag: each.tag, category: each.target.trim() })) }
      : {}),
    replace: replacing,
  });
  // Updating requesters touches only who is ticked or unticked.
  const dropped = $derived(requesters.filter((each) => each.added && !each.chosen));
  const updating = $derived(byRequester && replacing);
  const changed = $derived(dropped.length > 0 || chosen.some((each) => !each.added));
  const ready = $derived(
    byRequester
      ? chosen.length > 0 && chosen.every((each) => each.target.trim()) && (!updating || changed)
      : category.trim() !== '' && (template.parameter !== 'languages' || languages.length > 0),
  );
  const action = $derived(
    updating ? 'TemplateUpdate' : replacing ? 'TemplateReplace' : 'TemplateAdd',
  );

  // Asked again when a field is left, so a category typed letter by letter asks once.
  let asked = $state(0);
  const ask = () => asked++;

  // Waits a moment, so boxes ticked in a row ask once.
  const settled = (signal: AbortSignal) =>
    new Promise<void>((resolve, reject) => {
      const timer = setTimeout(resolve, 300);
      signal.addEventListener(
        'abort',
        () => {
          clearTimeout(timer);
          reject(signal.reason);
        },
        { once: true },
      );
    });

  const preview = createAsync(
    async (signal) => {
      const wanted = untrack(() => (ready ? request : null));
      if (!wanted) return null;
      await settled(signal);
      return api.previewTemplate(template.id, wanted, signal);
    },
    () => asked,
  );

  const issues = $derived(
    (preview.data?.rules ?? []).flatMap((planned) =>
      planned.issues.map((issue) => ({
        ...issue,
        message: t('TemplateIssue', { rule: planned.rule.name, issue: issue.message }),
      })),
    ),
  );
  const reach = $derived(preview.data?.reach ?? null);
  // Matched by the template, decided by a rule of the user's placed before.
  const keptElsewhere = $derived(
    (preview.data?.taken_by ?? []).reduce((sum, taken) => sum + taken.titles, 0),
  );
  const matched = $derived((reach?.titles ?? 0) + keptElsewhere);
  // The tones the simulation gives the same outcomes.
  const OUTCOMES: Record<DecisionAction, { label: string; badge: string }> = {
    move: { label: 'TemplateOutcomeMove', badge: 'badge-warning' },
    none: { label: 'TemplateOutcomeNone', badge: 'badge-success' },
    skip: { label: 'TemplateOutcomeSkip', badge: 'badge-danger' },
  };
  const segments = $derived(
    reach
      ? [
          { key: 'move', count: reach.to_move, label: 'TemplateOutcomeMove', tone: 'warning' },
          { key: 'none', count: reach.in_place, label: 'TemplateOutcomeNone', tone: 'success' },
          { key: 'skip', count: reach.unmapped, label: 'TemplateOutcomeSkip', tone: 'danger' },
          { key: 'kept', count: keptElsewhere, label: 'TemplateOutcomeKept', tone: undefined },
        ].filter((segment) => segment.count > 0)
      : [],
  );

  async function add(event: SubmitEvent) {
    event.preventDefault();
    if (!ready || adding) return;
    const question = updating
      ? dropped.length > 0 &&
        t('TemplateRequestersRemoveQuestion', {
          names: dropped.map((each) => each.name).join(t('ListSeparator')),
        })
      : replacing && t('TemplateReplaceQuestion', { template: template.name });
    const confirmLabel = updating ? 'TemplateRequestersRemove' : 'TemplateReplace';
    if (question && !(await askConfirmation(question, confirmLabel))) return;
    adding = true;
    failure = null;
    try {
      onAdded(await api.importTemplate(template.id, request));
    } catch (err) {
      failure = describeError(err);
    } finally {
      adding = false;
    }
  }
</script>

<Modal label={template.name} {onClose} maxWidth={760} initialFocus={firstField}>
  <form novalidate onsubmit={add}>
    <h2 class="modal-title">{template.name}</h2>
    <p class="text-muted text-base mb-3">{template.description}</p>
    {#if template.needs.length > 0}
      <div class="banner banner-warning">
        <span>
          {t('TemplateNeeds', {
            sources: template.needs.map((source) => source.name).join(t('ListSeparator')),
          })}
          <a class="text-link" href={href('/sources')}>{t('TemplateNeedsLink')}</a>
        </span>
      </div>
    {/if}
    <ErrorBanner message={failure} onDismiss={() => (failure = null)} />

    {#if byRequester}
      <fieldset class="form-group">
        <legend>{t('TemplateRequesters')}</legend>
        {#if requesters.length === 0}
          <p class="form-hint" id="template-requesters-none" tabindex="-1">
            {t('TemplateRequestersNone')}
          </p>
        {:else}
          <p class="form-hint">
            {t(updating ? 'TemplateRequestersUpdateHelp' : 'TemplateRequestersHelp')}
          </p>
          {#each requesters as requester, index (requester.tag)}
            <div class="form-row">
              <label class="check-option flex-fill-240">
                <input
                  id="template-requester-check-{index}"
                  type="checkbox"
                  bind:checked={requester.chosen}
                  onchange={ask}
                />
                <span
                  >{t('TemplateRequesterOption', {
                    name: requester.name,
                    count: requester.titles,
                  })}</span
                >
              </label>
              <div class="form-group flex-fill-240">
                <label class="form-label" for="template-requester-{index}">
                  {t('TemplateRequesterCategory', { name: requester.name })}
                </label>
                <input
                  id="template-requester-{index}"
                  class="form-input"
                  list="template-categories"
                  disabled={!requester.chosen || requester.added}
                  bind:value={requester.target}
                  onchange={ask}
                />
              </div>
            </div>
          {/each}
        {/if}
      </fieldset>
    {:else}
      <div class="form-row">
        <div class="form-group flex-fill-240">
          <label class="form-label" for="template-category">{t('Category')}</label>
          <input
            id="template-category"
            class="form-input"
            list="template-categories"
            aria-describedby="template-category-hint"
            bind:value={category}
            onchange={ask}
          />
          <span class="form-hint" id="template-category-hint">{t('TemplateCategoryHint')}</span>
        </div>
      </div>
    {/if}
    <datalist id="template-categories">
      {#each categories as known (known.id)}
        <option value={known.name}></option>
      {/each}
    </datalist>

    {#if template.parameter === 'languages'}
      <div class="form-group">
        <label class="form-label" for="template-languages">{t('TemplateLanguages')}</label>
        <ValuePicker
          id="template-languages"
          label={t('TemplateLanguages')}
          values={languages}
          options={languageOptions}
          describedBy="template-languages-hint"
          onChange={(values) => {
            languages = values;
            ask();
          }}
        />
        <span class="form-hint" id="template-languages-hint">{t('TemplateLanguagesHint')}</span>
      </div>
    {/if}

    <ErrorBanner message={preview.error} onRetry={() => void preview.reload()} />
    {#if ready}
      <div class="card card-inset mt-4">
        <div class="card-header">
          <CardTitle card="TemplateEffect" level={3}>{t('TemplateEffect')}</CardTitle>
        </div>
        <div class="template-effect">
          <div role="status">
            {#if reach && matched > 0}
              <p class="template-reach">
                <span class="template-reach-value">{formatCount(matched, i18n.language)}</span>
                <span class="template-reach-label">{t('TemplateReachCaption')}</span>
              </p>
            {:else if reach}
              <p class="text-base">{t('TemplateReachNone')}</p>
            {:else if preview.loading}
              <p class="text-base text-muted">{t('TemplatePreviewing')}</p>
            {/if}
          </div>
          {#if preview.data && reach}
            {#if segments.length > 0}
              <div class="template-bar" aria-hidden="true">
                {#each segments as segment (segment.key)}
                  <span class="is-{segment.key}" style="flex-grow: {segment.count}"></span>
                {/each}
              </div>
              <div class="template-metrics">
                {#each segments as segment (segment.key)}
                  <Stat label={t(segment.label)} value={segment.count} tone={segment.tone} />
                {/each}
              </div>
            {/if}
            {#if preview.data.taken_by.length > 0}
              <div class="template-kept">
                <p class="template-kept-title">{t('TemplateKeptBy')}</p>
                <ul>
                  {#each preview.data.taken_by as taken (taken.rule_id)}
                    <li>
                      <span>{taken.rule_name}</span>
                      <span class="template-kept-count">
                        {formatCount(taken.titles, i18n.language)}
                      </span>
                    </li>
                  {/each}
                </ul>
                <p class="form-hint">{t('TemplateTakenHint')}</p>
              </div>
            {/if}
            {#each preview.data.creates as created (created)}
              <div class="banner banner-warning">
                <span>
                  {t('TemplateCreates', { category: created })}
                  <a class="text-link" href={href('/categories')}>{t('TemplateCreatesLink')}</a>
                </span>
              </div>
            {/each}
            {#each issues as issue, index (index)}
              <div class="banner {issue.severity === 'error' ? 'banner-danger' : 'banner-warning'}">
                <span>{issue.message}</span>
              </div>
            {/each}
            {#if preview.data.titles.length > 0}
              <TableRegion label={t('TemplateTitles')}>
                <table>
                  <caption class="visually-hidden">{t('TemplateTitles')}</caption>
                  <thead>
                    <tr>
                      <th>{t('Media')}</th>
                      {#if byRequester}
                        <th>{t('Category')}</th>
                      {/if}
                      <th>{t('TemplateOutcome')}</th>
                    </tr>
                  </thead>
                  <tbody>
                    {#each preview.data.titles as title (title.media_id)}
                      <tr>
                        <td>{title.media_title}</td>
                        {#if byRequester}
                          <td><span class="badge badge-value">{title.category}</span></td>
                        {/if}
                        <td>
                          <span class="badge {OUTCOMES[title.action].badge}">
                            {t(OUTCOMES[title.action].label)}
                          </span>
                        </td>
                      </tr>
                    {/each}
                  </tbody>
                </table>
                {#if reach.titles > preview.data.titles.length}
                  <p class="text-muted mt-2 text-sm">
                    {t('TemplateTitlesShown', {
                      shown: preview.data.titles.length,
                      total: reach.titles,
                    })}
                  </p>
                {/if}
              </TableRegion>
            {/if}
            {#if reach.to_move > 0}
              <p class="template-note">
                <Info size={14} aria-hidden="true" />
                <span>{t('TemplateNothingMoves')}</span>
              </p>
            {/if}
          {/if}
        </div>
      </div>
    {/if}

    <div class="dialog-actions">
      <button type="button" class="btn btn-secondary" onclick={onClose}>{t('Cancel')}</button>
      <button type="submit" class="btn btn-primary" disabled={!ready || adding}>
        {t(action)}
      </button>
    </div>
  </form>
</Modal>
