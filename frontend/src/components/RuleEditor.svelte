<script lang="ts">
  import { tick } from 'svelte';
  import { FlaskConical } from '../lib/icons';
  import { ApiError, api } from '../api/client';
  import { conditionAppliesTo, defaultConditionValue, issueField } from '../api/conditions';
  import { toggled } from '../api/format';
  import type {
    ValidationIssue,
    Category,
    ConditionCatalog,
    Instance,
    LibraryFacets,
    MatchMode,
    RuleDraft,
    RuleMediaType,
    RulePreview,
  } from '../api/types';
  import { describeError } from '../lib/async.svelte';
  import { askConfirmation } from '../lib/confirm.svelte';
  import { handFocus } from '../lib/focus';
  import { t } from '../lib/i18n.svelte';
  import { holdUnsaved } from '../lib/unsaved.svelte';
  import { access } from '../lib/access.svelte';
  import ConditionList from './ConditionList.svelte';
  import ErrorBanner from './ErrorBanner.svelte';
  import FieldIssues from './FieldIssues.svelte';
  import Modal from './Modal.svelte';
  import PreviewPanel from './PreviewPanel.svelte';

  let {
    draft: initial,
    ruleId,
    categories,
    instances,
    catalog,
    onClose,
    onSaved,
    knownFacets = null,
    returnFocus,
  }: {
    draft: RuleDraft;
    ruleId?: string;
    categories: Category[];
    /** What a rule can be kept to. None ticked is every instance. */
    instances: Instance[];
    catalog: ConditionCatalog;
    onClose: () => void;
    onSaved: (message: string) => Promise<void>;
    /** The library's facets, when the parent already holds them. */
    knownFacets?: LibraryFacets | null;
    /** Where the focus goes on closing when no control opened the editor. */
    returnFocus?: string;
  } = $props();

  // Once for both lists, and only if `Rules` holds none: each read aggregates the whole library.
  let fetched = $state<LibraryFacets | null>(null);
  let facetsError = $state<string | null>(null);
  const facets = $derived(knownFacets ?? fetched);
  const facetsLoading = $derived(facets === null && facetsError === null);
  $effect(() => {
    if (knownFacets !== null) return;
    const closing = new AbortController();
    api
      .getLibraryFacets(closing.signal)
      .then((loaded) => (fetched = loaded))
      .catch((cause) => {
        if (!closing.signal.aborted) facetsError = describeError(cause);
      });
    return () => closing.abort();
  });

  // Read once: the editor owns its copy, and a `$derived` would drop the edits on each render.
  // svelte-ignore state_referenced_locally
  let draft = $state<RuleDraft>({ ...initial });
  // The draft as it opened, as text: whether closing drops any work.
  // svelte-ignore state_referenced_locally
  const opened = JSON.stringify(initial);
  const unsaved = $derived(JSON.stringify(draft) !== opened);
  holdUnsaved(
    () => unsaved,
    () => t('ConfirmDiscardRule'),
  );

  /** Escape, the close button and Cancel: a half-written rule is asked about first. */
  async function close() {
    if (unsaved && !(await askConfirmation(t('ConfirmDiscardRule'), 'DiscardChanges'))) return;
    onClose();
  }
  let preview = $state<RulePreview | null>(null);
  let busy = $state<'save' | 'preview' | null>(null);
  let error = $state<string | null>(null);

  // The server's verdict on the draft, asked a moment after the last edit. A failure shows no
  // banner: saving asks the same question and reports its answer.
  let issues = $state<ValidationIssue[]>([]);
  /**
   * Nothing is said about a new form nobody has touched, whose empty fields speak already. A
   * saved rule speaks on open, where an issue is news. A latch, since adding and removing a
   * condition would make the form untouched again and hide the verdict with Save lit.
   */
  // Read once, like the draft itself.
  // svelte-ignore state_referenced_locally
  let edited = $state(Boolean(ruleId));
  const VALIDATE_DELAY_MS = 400;
  $effect(() => {
    const snapshot = JSON.stringify(draft);
    // A question about a draft edited since is abandoned, not left to run.
    const asking = new AbortController();
    const timer = setTimeout(() => {
      const candidate = JSON.parse(snapshot) as RuleDraft;
      api
        .validateRule(candidate, asking.signal)
        .then((result) => {
          if (JSON.stringify(draft) === snapshot) issues = result.issues;
        })
        .catch(() => {});
    }, VALIDATE_DELAY_MS);
    return () => {
      asking.abort();
      clearTimeout(timer);
    };
  });
  /**
   * A condition just added has no value yet. Said before the reader could pick one, it reads as
   * a mistake they did not make and holds Save, so it waits for a Save press.
   */
  const EMPTY_CONDITION = 'ValidationConditionEmpty';
  let submitted = $state(false);
  const shown = $derived(
    edited ? issues.filter((issue) => submitted || issue.key !== EMPTY_CONDITION) : [],
  );
  // Gated as the list is: a button disabled for a reason nobody is shown is a dead control.
  const blocking = $derived(shown.some((issue) => issue.severity === 'error'));

  // Each error is said under its control as well as in the summary.
  const errors = $derived(shown.filter((issue) => issue.severity === 'error'));
  const errorsOf = (field: string) => errors.filter((issue) => issueField(issue) === field);
  /** A field's own descriptions, then the errors said under it. */
  const describedBy = (field: string, ...own: string[]) =>
    [...own, errorsOf(field).length > 0 ? `${field}-issue` : ''].filter(Boolean).join(' ') ||
    undefined;
  const invalid = (field: string) => (errorsOf(field).length > 0 ? 'true' : undefined);

  /** Hands the focus to the control `field` names, the value of a condition for its row. */
  function focusField(field: string) {
    const element = document.getElementById(field);
    const fold = element?.closest('details');
    if (fold) fold.open = true;
    const control = element?.matches('input, select, textarea, button')
      ? element
      : (element?.querySelector<HTMLElement>('.condition-value :is(input, select, textarea)') ??
        element?.querySelector<HTMLElement>('input, select, textarea'));
    control?.focus();
  }

  /** On a refused Save, the first field in the form's order that an error is said under. */
  async function focusFirstError() {
    await tick();
    const first = errors
      .map((issue) => issueField(issue))
      .map((field) => (field ? document.getElementById(field) : null))
      .reduce<HTMLElement | null>(
        (earliest, element) =>
          element &&
          (!earliest ||
            element.compareDocumentPosition(earliest) & Node.DOCUMENT_POSITION_FOLLOWING)
            ? element
            : earliest,
        null,
      );
    if (first) focusField(first.id);
  }
  // Checked here: a fraction never reaches the live validation, since the
  // server refuses the draft before reading it, and Save would stay lit.
  const wholePriority = $derived(Number.isInteger(draft.priority));

  /** What is seldom changed, folded. A refusal of one of its fields unfolds it. */
  let more = $state(false);
  const moreRefused = $derived(
    !wholePriority ||
      errorsOf('rules-description').length > 0 ||
      errorsOf('rules-priority').length > 0,
  );
  $effect(() => {
    if (moreRefused) more = true;
  });
  const MATCH_MODES: { value: MatchMode; key: string }[] = [
    { value: 'all', key: 'MatchAll' },
    { value: 'any', key: 'MatchAny' },
  ];

  const specs = $derived(new Map(catalog.conditions.map((spec) => [spec.type, spec])));

  // Narrowed by media type, or a movie rule offers `season_count_over`, which cannot match.
  const addable = $derived(
    catalog.conditions.filter((spec) => conditionAppliesTo(spec, draft.media_type)),
  );

  /** Any edit invalidates the impact numbers on screen, and is what makes the
   * verdict due: every control that changes the draft calls this. */
  function touched() {
    preview = null;
    edited = true;
  }

  /**
   * Beside each condition, in step with it: a key that keeps its typed text and error with it
   * when one above is removed, and whether its field holds text it could not read. The draft
   * keeps only what a field could read, so Save waits for every such field.
   */
  let nextRow = 0;
  const row = () => ({ key: nextRow++, unreadable: false });
  // Built once from the draft as it opened, then kept in step by each edit.
  const rows = $state({
    conditions: draft.conditions.map(row),
    exclusions: draft.exclusions.map(row),
  });
  const unreadable = $derived(
    [...rows.conditions, ...rows.exclusions].filter((field) => field.unreadable).length,
  );

  function addCondition(list: 'conditions' | 'exclusions', type: string) {
    const spec = specs.get(type);
    if (!spec) return;
    draft[list] = [...draft[list], { type, value: defaultConditionValue(spec) }];
    rows[list] = [...rows[list], row()];
    touched();
  }

  // The values survive the swap: the question is the same, asked with the other quantifier.
  function retypeCondition(list: 'conditions' | 'exclusions', index: number, type: string) {
    draft[list] = draft[list].map((condition, i) =>
      i === index ? { ...condition, type } : condition,
    );
    touched();
  }

  function updateCondition(
    list: 'conditions' | 'exclusions',
    index: number,
    value: unknown,
    unreadableText: boolean,
  ) {
    draft[list] = draft[list].map((condition, i) =>
      i === index ? { ...condition, value } : condition,
    );
    const field = rows[list][index];
    if (field) field.unreadable = unreadableText;
    touched();
  }

  /** An empty list would read as every instance too, so none ticked is sent as `null`. */
  function keepTo(id: string, ticked: boolean) {
    const kept = toggled(draft.instance_ids ?? [], id, ticked);
    draft.instance_ids = kept.length > 0 ? kept : null;
    touched();
  }

  function removeCondition(list: 'conditions' | 'exclusions', index: number) {
    draft[list] = draft[list].filter((_, i) => i !== index);
    rows[list] = rows[list].filter((_, i) => i !== index);
    touched();
  }

  async function runPreview() {
    const pressed = document.activeElement as HTMLElement | null;
    busy = 'preview';
    error = null;
    try {
      preview = await api.previewRule(draft, ruleId);
    } catch (err) {
      error = describeError(err);
    } finally {
      busy = null;
      void handFocus(pressed);
    }
  }

  async function save(event: SubmitEvent) {
    event.preventDefault();
    const pressed = document.activeElement as HTMLElement | null;
    // Fetched, not read from `issues`, which is empty just after mount and when the debounced
    // call failed. An empty condition held back is asked again, its value maybe just picked.
    submitted = true;
    if (!edited || issues.some((issue) => issue.key === EMPTY_CONDITION)) {
      edited = true;
      const verdict = await api.validateRule(draft).catch(() => null);
      if (verdict) issues = verdict.issues;
    }
    if (issues.some((issue) => issue.severity === 'error')) {
      await focusFirstError();
      return;
    }
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
      void handFocus(pressed);
    }
  }
</script>

<Modal
  title={t(ruleId ? 'EditRule' : 'CreateRule')}
  onClose={() => void close()}
  maxWidth={860}
  maxHeight="88vh"
  initialFocus="rules-rule-name"
  {returnFocus}
>
  <ErrorBanner message={error} onDismiss={() => (error = null)} />

  <!-- `novalidate`: the browser's bubble speaks the browser's language and fires before the
       submit handler, over the translated verdict. `required` stays, for its semantics. -->
  <form novalidate onsubmit={save}>
    <div class="form-group">
      <label class="form-label" for="rules-rule-name">{t('RuleName')}</label>
      <input
        id="rules-rule-name"
        class="form-input"
        placeholder={t('RuleNamePlaceholder')}
        bind:value={draft.name}
        oninput={touched}
        required
        aria-invalid={invalid('rules-rule-name')}
        aria-describedby={describedBy('rules-rule-name')}
      />
      <FieldIssues field="rules-rule-name" issues={errorsOf('rules-rule-name')} />
    </div>

    <div class="form-row">
      <div class="form-group flex-fill-200">
        <label class="form-label" for="rules-target-category">{t('SendsTo')}</label>
        <select
          id="rules-target-category"
          class="form-select"
          bind:value={draft.target_category}
          aria-invalid={invalid('rules-target-category')}
          aria-describedby={describedBy('rules-target-category')}
          onchange={touched}
        >
          {#each categories as category (category.id)}
            <option value={category.name}>
              {category.root_folder_count === 0
                ? t('SideBySide', { first: category.name, second: t('NoRootFolderMappedSuffix') })
                : category.name}
            </option>
          {/each}
        </select>
        <FieldIssues field="rules-target-category" issues={errorsOf('rules-target-category')} />
      </div>

      <div class="form-group flex-fill-200">
        <label class="form-label" for="rules-applies-to">{t('AppliesTo')}</label>
        <select
          id="rules-applies-to"
          class="form-select"
          aria-invalid={invalid('rules-applies-to')}
          aria-describedby={describedBy('rules-applies-to')}
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
        <FieldIssues field="rules-applies-to" issues={errorsOf('rules-applies-to')} />
      </div>
    </div>

    {#if instances.length > 0}
      <fieldset class="form-group">
        <legend>{t('Instances')}</legend>
        <p class="form-hint">{t('RuleInstancesHelp')}</p>
        {#each instances as instance (instance.id)}
          <label class="check-option">
            <input
              type="checkbox"
              checked={draft.instance_ids?.includes(instance.id) ?? false}
              onchange={(event) => keepTo(instance.id, event.currentTarget.checked)}
            />
            <span>{instance.name}</span>
          </label>
        {/each}
      </fieldset>
    {/if}

    {#snippet logic()}
      <div class="segmented" role="group" aria-label={t('ConditionLogic')}>
        {#each MATCH_MODES as mode (mode.value)}
          <button
            type="button"
            class="btn btn-secondary btn-sm"
            aria-pressed={draft.match_mode === mode.value}
            onclick={() => {
              draft.match_mode = mode.value;
              touched();
            }}
          >
            {t(mode.key)}
          </button>
        {/each}
      </div>
    {/snippet}

    <ConditionList
      title={t('Conditions')}
      control={logic}
      list="conditions"
      conditions={draft.conditions}
      keys={rows.conditions.map((field) => field.key)}
      specs={catalog.conditions}
      {addable}
      {facets}
      {facetsLoading}
      {facetsError}
      scales={catalog.score_scales ?? []}
      {errorsOf}
      onAdd={(type) => addCondition('conditions', type)}
      onRetype={(index, type) => retypeCondition('conditions', index, type)}
      onUpdate={(index, value, unreadableText) =>
        updateCondition('conditions', index, value, unreadableText)}
      onRemove={(index) => removeCondition('conditions', index)}
    />

    <ConditionList
      title={t('ExclusionsLabel')}
      list="exclusions"
      conditions={draft.exclusions}
      keys={rows.exclusions.map((field) => field.key)}
      specs={catalog.conditions}
      {addable}
      {facets}
      {facetsLoading}
      {facetsError}
      scales={catalog.score_scales ?? []}
      {errorsOf}
      onAdd={(type) => addCondition('exclusions', type)}
      onRetype={(index, type) => retypeCondition('exclusions', index, type)}
      onUpdate={(index, value, unreadableText) =>
        updateCondition('exclusions', index, value, unreadableText)}
      onRemove={(index) => removeCondition('exclusions', index)}
    />

    <details class="rule-more" bind:open={more}>
      <summary>
        {t('WithNote', {
          text: t('RuleMore'),
          note: [
            t('Description'),
            t('SideBySide', { first: t('Priority'), second: String(draft.priority) }),
            t(draft.enabled ? 'RuleEnabled' : 'RuleDisabled'),
          ].join(t('ListSeparator')),
        })}
      </summary>
      <div class="form-group">
        <label class="form-label" for="rules-description">
          {t('WithNote', { text: t('Description'), note: t('Optional') })}
        </label>
        <input
          id="rules-description"
          class="form-input"
          placeholder={t('RuleDescriptionPlaceholder')}
          value={draft.description ?? ''}
          aria-invalid={invalid('rules-description')}
          aria-describedby={describedBy('rules-description')}
          oninput={(event) => {
            draft.description = event.currentTarget.value;
            touched();
          }}
        />
        <FieldIssues field="rules-description" issues={errorsOf('rules-description')} />
      </div>

      <div class="form-group">
        <label class="form-label" for="rules-priority">{t('Priority')}</label>
        <div class="field-beside">
          <input
            id="rules-priority"
            type="number"
            step="1"
            class="form-input flex-fixed-120"
            aria-invalid={wholePriority ? invalid('rules-priority') : 'true'}
            aria-describedby={wholePriority
              ? describedBy('rules-priority', 'rules-priority-help')
              : 'rules-priority-help rules-priority-error'}
            value={draft.priority}
            oninput={(event) => {
              draft.priority = Number(event.currentTarget.value);
              touched();
            }}
          />
          <p id="rules-priority-help" class="form-hint">{t('PriorityHint')}</p>
        </div>
        {#if !wholePriority}
          <p id="rules-priority-error" class="field-error">{t('EnterWholeNumber')}</p>
        {:else}
          <FieldIssues field="rules-priority" issues={errorsOf('rules-priority')} />
        {/if}
      </div>

      <div class="form-group">
        <label class="flex items-center gap-2 text-base">
          <input type="checkbox" bind:checked={draft.enabled} onchange={touched} />
          {t('RuleEnabled')}
        </label>
      </div>
    </details>

    {#if preview}
      <PreviewPanel {preview} />
    {/if}

    <!-- Always here: a live region added with its content announces nothing, and on a Save
         press the button going quiet is the only other visible effect. -->
    <div aria-live="polite">
      {#if shown.length}
        <ul class="validation-issues" aria-label={t('ValidationIssues')}>
          {#each shown as issue, index (index)}
            {@const field = issueField(issue)}
            <li class="banner {issue.severity === 'error' ? 'banner-danger' : 'banner-warning'}">
              {#if field}
                <button type="button" class="issue-link" onclick={() => focusField(field)}>
                  <span>{issue.message}</span>
                </button>
              {:else}
                {issue.message}
              {/if}
            </li>
          {/each}
        </ul>
      {/if}
    </div>

    <div class="dialog-actions">
      <button type="button" class="btn btn-secondary" onclick={() => void close()}>
        {t('Cancel')}
      </button>
      <!-- A reader is refused both: the server keeps them to whoever edits a rule. -->
      {#if !access.readOnly}
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
      {/if}
    </div>
  </form>
</Modal>
