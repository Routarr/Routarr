<script lang="ts">
  import { tick } from 'svelte';
  import { FlaskConical } from '../lib/icons';
  import { ApiError, api } from '../api/client';
  import { conditionAppliesTo, defaultConditionValue } from '../api/conditions';
  import type {
    ValidationIssue,
    Category,
    ConditionCatalog,
    LibraryFacets,
    MatchMode,
    RuleDraft,
    RuleMediaType,
    RulePreview,
  } from '../api/types';
  import { describeError } from '../lib/async.svelte';
  import { askConfirmation } from '../lib/confirm.svelte';
  import { t } from '../lib/i18n.svelte';
  import ConditionList from './ConditionList.svelte';
  import ErrorBanner from './ErrorBanner.svelte';
  import Modal from './Modal.svelte';
  import PreviewPanel from './PreviewPanel.svelte';

  let {
    draft: initial,
    ruleId,
    categories,
    catalog,
    onClose,
    onSaved,
    knownFacets = null,
    returnFocus,
  }: {
    draft: RuleDraft;
    ruleId?: string;
    categories: Category[];
    catalog: ConditionCatalog;
    onClose: () => void;
    onSaved: (message: string) => Promise<void>;
    /** The library's facets, when the parent already holds them. */
    knownFacets?: LibraryFacets | null;
    /** Where the focus goes on closing when no control opened the editor. */
    returnFocus?: string;
  } = $props();

  // What the library holds, so each condition can offer its values rather than
  // ask for an exact spelling. Fetched here rather than by the picker: both
  // condition lists want the same answer, and one editor is one request at
  // most. `Rules` already holds the facets for its panel, and the editor asking
  // again would aggregate the whole library a second time every time it opens,
  // so it fetches only when the parent has nothing to hand over.
  // svelte-ignore state_referenced_locally
  let facets = $state<LibraryFacets | null>(knownFacets);
  // svelte-ignore state_referenced_locally
  let facetsLoading = $state(knownFacets === null);
  let facetsError = $state<string | null>(null);
  $effect(() => {
    if (knownFacets !== null) return;
    api
      .getLibraryFacets()
      .then((loaded) => (facets = loaded))
      .catch((cause) => (facetsError = describeError(cause)))
      .finally(() => (facetsLoading = false));
  });

  // Read once, deliberately: this is the *initial* draft. The editor owns its
  // copy from then on, and a `$derived` would throw the user's edits away every
  // time the parent re-rendered.
  // svelte-ignore state_referenced_locally
  let draft = $state<RuleDraft>({ ...initial });
  // The draft as it opened, as text: whether closing drops any work.
  // svelte-ignore state_referenced_locally
  const opened = JSON.stringify(initial);
  const unsaved = $derived(JSON.stringify(draft) !== opened);

  /** Escape, the close button and Cancel: a half-written rule is asked about first. */
  async function close() {
    if (unsaved && !(await askConfirmation(t('ConfirmDiscardRule'), 'DiscardChanges'))) return;
    onClose();
  }
  let preview = $state<RulePreview | null>(null);
  let busy = $state<'save' | 'preview' | null>(null);
  let error = $state<string | null>(null);

  // What the server would say about the draft as it stands, asked a moment
  // after the last edit. Advisory: a failed request here is not worth a banner,
  // since saving asks the same question and reports its answer.
  let issues = $state<ValidationIssue[]>([]);
  /**
   * Nothing is said about a form nobody has touched.
   *
   * Both complaints a new rule draws on open (no name, no condition) are
   * about a form that is merely empty, and the empty fields say so already.
   * From the first edit onwards it speaks, which is what this validation is
   * for: a value that cannot match, a condition its media type does not allow,
   * are worth knowing while the rule is being built.
   *
   * A rule that already exists speaks on open instead: it was saved once, so an
   * issue on it is news about something that changed underneath.
   *
   * A latch, not a comparison against the opening draft: adding a condition and
   * removing it again would otherwise make the form untouched a second time,
   * and the verdict would vanish with Save lit on an unsaveable rule.
   */
  // Read once, like the draft itself.
  // svelte-ignore state_referenced_locally
  let edited = $state(Boolean(ruleId));
  const VALIDATE_DELAY_MS = 400;
  $effect(() => {
    const snapshot = JSON.stringify(draft);
    const timer = setTimeout(() => {
      const candidate = JSON.parse(snapshot) as RuleDraft;
      api
        .validateRule(candidate)
        .then((result) => {
          if (JSON.stringify(draft) === snapshot) issues = result.issues;
        })
        .catch(() => {});
    }, VALIDATE_DELAY_MS);
    return () => clearTimeout(timer);
  });
  /**
   * A condition just added has no value yet. Said before the reader had a
   * chance to pick one, it reads as a mistake they did not make and holds Save
   * for a reason that is only the order of their clicks, so it waits for a
   * Save press, which asks the question outright.
   */
  const EMPTY_CONDITION = 'ValidationConditionEmpty';
  let submitted = $state(false);
  const shown = $derived(
    edited ? issues.filter((issue) => submitted || issue.key !== EMPTY_CONDITION) : [],
  );
  // Gated on the same flag as the list: a button disabled by a reason nobody is
  // shown is a dead control, which is worse than the premature complaint.
  const blocking = $derived(shown.some((issue) => issue.severity === 'error'));
  // Checked here: a fraction never reaches the live validation, since the
  // server refuses the draft before reading it, and Save would stay lit.
  const wholePriority = $derived(Number.isInteger(draft.priority));

  const specs = $derived(new Map(catalog.conditions.map((spec) => [spec.type, spec])));

  // Narrowed by the rule's own media type, and recomputed when it changes: an
  // unnarrowed picker would offer `season_count_over` on a movie rule, a
  // condition that cannot match, on a screen whose whole job is to say what
  // will.
  const addable = $derived(
    catalog.conditions.filter((spec) => conditionAppliesTo(spec, draft.media_type)),
  );

  /** Any edit invalidates the impact numbers on screen, and is what makes the
   * verdict due: every control that changes the draft calls this. */
  function touched() {
    preview = null;
    edited = true;
  }

  // A condition field that names what it could not read: the draft holds only
  // what it could, so saving would drop the rest unseen. Counted on the form,
  // where every condition field draws, after each edit has redrawn it.
  let form = $state<HTMLFormElement | null>(null);
  let unreadable = $state(0);
  async function recount() {
    await tick();
    unreadable = form?.querySelectorAll('.condition-row [aria-invalid="true"]').length ?? 0;
  }

  function addCondition(list: 'conditions' | 'exclusions', type: string) {
    const spec = specs.get(type);
    if (!spec) return;
    draft[list] = [...draft[list], { type, value: defaultConditionValue(spec) }];
    touched();
  }

  // The values survive the swap: the question is the same one, asked with the
  // other quantifier, and losing a selection to a turn of the selector would be
  // the surest way to stop anyone using it.
  function retypeCondition(list: 'conditions' | 'exclusions', index: number, type: string) {
    draft[list] = draft[list].map((condition, i) =>
      i === index ? { ...condition, type } : condition,
    );
    touched();
  }

  function updateCondition(list: 'conditions' | 'exclusions', index: number, value: unknown) {
    draft[list] = draft[list].map((condition, i) =>
      i === index ? { ...condition, value } : condition,
    );
    touched();
  }

  function removeCondition(list: 'conditions' | 'exclusions', index: number) {
    draft[list] = draft[list].filter((_, i) => i !== index);
    touched();
    void recount();
  }

  async function runPreview() {
    busy = 'preview';
    error = null;
    try {
      preview = await api.previewRule(draft, ruleId);
    } catch (err) {
      error = describeError(err);
    } finally {
      busy = null;
    }
  }

  async function save(event: SubmitEvent) {
    event.preventDefault();
    // Pressing Save on an untouched form is also asking. The answer is fetched
    // rather than read from `issues`, which is empty for the first few hundred
    // milliseconds after mount and stays empty if the debounced call failed,
    // and falling through then would send a draft the server only refuses.
    // An empty condition held back until now is asked about again: the value
    // may have been picked within the last debounce.
    submitted = true;
    if (!edited || issues.some((issue) => issue.key === EMPTY_CONDITION)) {
      edited = true;
      const verdict = await api.validateRule(draft).catch(() => null);
      if (verdict) issues = verdict.issues;
    }
    if (issues.some((issue) => issue.severity === 'error')) return;
    busy = 'save';
    error = null;
    try {
      if (ruleId) {
        await api.updateRule(ruleId, draft);
        await onSaved(t('RuleUpdated'));
      } else {
        await api.createRule(draft);
        await onSaved(t('RuleCreated'));
      }
    } catch (err) {
      error =
        err instanceof ApiError && err.status === 400
          ? t('RuleRejected', { message: err.message })
          : describeError(err);
    } finally {
      busy = null;
    }
  }
</script>

<Modal
  label={t(ruleId ? 'EditRule' : 'CreateRule')}
  onClose={() => void close()}
  maxWidth={860}
  maxHeight="88vh"
  initialFocus="rules-rule-name"
  {returnFocus}
>
  <div class="modal-header">
    <h2 class="modal-title">{t(ruleId ? 'EditRule' : 'CreateRule')}</h2>
    <button
      class="btn btn-secondary btn-sm"
      onclick={() => void close()}
      aria-label={t('Dismiss')}
      title={t('Dismiss')}
    >
      ✕
    </button>
  </div>

  <ErrorBanner message={error} onDismiss={() => (error = null)} />

  <!-- `novalidate` because the browser's own bubble renders in the *browser's*
       language whatever `ui_language` says, and fires before the submit
       handler, so it speaks over the editor's translated verdict rather than
       instead of it. `required` stays: it is the semantics, not the bubble.
       The same reasoning keeps `window.confirm` out of this codebase. -->
  <form bind:this={form} novalidate onsubmit={save} oninput={() => void recount()}>
    <div class="form-group">
      <label class="form-label" for="rules-rule-name">{t('RuleName')}</label>
      <input
        id="rules-rule-name"
        class="form-input"
        placeholder={t('RuleNamePlaceholder')}
        bind:value={draft.name}
        oninput={touched}
        required
      />
    </div>

    <div class="form-group">
      <label class="form-label" for="rules-description">
        {t('Description')} ({t('Optional')})
      </label>
      <input
        id="rules-description"
        class="form-input"
        placeholder={t('RuleDescriptionPlaceholder')}
        value={draft.description ?? ''}
        oninput={(event) => {
          draft.description = event.currentTarget.value;
          touched();
        }}
      />
    </div>

    <div class="form-row">
      <div class="form-group flex-fill-200">
        <label class="form-label" for="rules-target-category">{t('TargetCategory')}</label>
        <select
          id="rules-target-category"
          class="form-select"
          bind:value={draft.target_category}
          onchange={touched}
        >
          {#each categories as category (category.id)}
            <option value={category.name}>
              {category.name}{category.root_folder_count === 0
                ? ` ${t('NoRootFolderMappedSuffix')}`
                : ''}
            </option>
          {/each}
        </select>
      </div>

      <div class="form-group flex-fill-200">
        <label class="form-label" for="rules-applies-to">{t('AppliesTo')}</label>
        <select
          id="rules-applies-to"
          class="form-select"
          value={draft.media_type}
          onchange={(event) => {
            draft.media_type = event.currentTarget.value as RuleMediaType;
            touched();
          }}
        >
          <option value="both">{t('Both')}</option>
          <option value="movie">{t('MoviesOnly')}</option>
          <option value="series">{t('SeriesOnly')}</option>
        </select>
      </div>

      <div class="form-group flex-fixed-120">
        <label class="form-label" for="rules-priority">{t('Priority')}</label>
        <input
          id="rules-priority"
          type="number"
          step="1"
          class="form-input"
          aria-invalid={wholePriority ? undefined : 'true'}
          aria-describedby={wholePriority ? undefined : 'rules-priority-error'}
          value={draft.priority}
          oninput={(event) => {
            draft.priority = Number(event.currentTarget.value);
            touched();
          }}
          title={t('PriorityHint')}
        />
        {#if !wholePriority}
          <p id="rules-priority-error" class="field-error">{t('EnterWholeNumber')}</p>
        {/if}
      </div>
    </div>

    <div class="form-row">
      <div class="form-group flex-fill-240">
        <label class="form-label" for="rules-condition-logic">{t('ConditionLogic')}</label>
        <select
          id="rules-condition-logic"
          class="form-select"
          value={draft.match_mode}
          onchange={(event) => {
            draft.match_mode = event.currentTarget.value as MatchMode;
            touched();
          }}
        >
          <option value="all">{t('MatchAll')}</option>
          <option value="any">{t('MatchAny')}</option>
        </select>
      </div>

      <div class="form-group flex-fill-240">
        <label class="flex items-center gap-2 text-base">
          <input type="checkbox" bind:checked={draft.enabled} onchange={touched} />
          {t('Enabled')}
        </label>
      </div>
    </div>

    <ConditionList
      title={t(draft.match_mode === 'any' ? 'ConditionsAnyOf' : 'ConditionsAllOf')}
      list="conditions"
      conditions={draft.conditions}
      specs={catalog.conditions}
      {addable}
      {facets}
      {facetsLoading}
      {facetsError}
      onAdd={(type) => addCondition('conditions', type)}
      onRetype={(index, type) => retypeCondition('conditions', index, type)}
      onUpdate={(index, value) => updateCondition('conditions', index, value)}
      onRemove={(index) => removeCondition('conditions', index)}
    />

    <ConditionList
      title={t('ExclusionsLabel')}
      list="exclusions"
      conditions={draft.exclusions}
      specs={catalog.conditions}
      {addable}
      {facets}
      {facetsLoading}
      {facetsError}
      onAdd={(type) => addCondition('exclusions', type)}
      onRetype={(index, type) => retypeCondition('exclusions', index, type)}
      onUpdate={(index, value) => updateCondition('exclusions', index, value)}
      onRemove={(index) => removeCondition('exclusions', index)}
    />

    {#if preview}
      <PreviewPanel {preview} />
    {/if}

    <!-- The region is always here so that filling it is a *change* a live
         region reports. Added at the same time as its content, it announces
         nothing. That matters on the Save press, whose only other visible
         effect is the button going quiet. -->
    <div aria-live="polite">
      {#if shown.length}
        <ul class="validation-issues" aria-label={t('ValidationIssues')}>
          {#each shown as issue, index (index)}
            <li class="banner {issue.severity === 'error' ? 'banner-danger' : 'banner-warning'}">
              {issue.message}
            </li>
          {/each}
        </ul>
      {/if}
    </div>

    <div class="dialog-actions">
      <button type="button" class="btn btn-secondary" onclick={() => void close()}>
        {t('Cancel')}
      </button>
      <div class="flex gap-2">
        <button
          type="button"
          class="btn btn-secondary"
          onclick={runPreview}
          disabled={busy !== null || draft.conditions.length === 0 || !wholePriority}
        >
          <FlaskConical size={16} />
          {busy === 'preview' ? t('Simulating') : t('PreviewImpact')}
        </button>
        <button
          type="submit"
          class="btn btn-primary"
          disabled={busy !== null || blocking || !wholePriority || unreadable > 0}
        >
          {busy === 'save' ? t('Saving') : t('SaveRule')}
        </button>
      </div>
    </div>
  </form>
</Modal>
