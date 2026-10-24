<script lang="ts">
  import type { ConditionSpec, Facet, ScoreScale } from '../api/types';
  import {
    MAX_RUNTIME,
    MIN_YEAR,
    maxYear,
    parseNumberList,
    rejectedNumbers,
    keyOfKind,
    parseScoreBound,
    parseStringList,
    parseWholeBound,
  } from '../api/conditions';
  import { t } from '../lib/i18n.svelte';
  import ValuePicker from './ValuePicker.svelte';

  let {
    spec,
    value,
    suggestions = [],
    suggestionsLoading = false,
    suggestionsError = null,
    describedBy,
    scales = [],
    onChange,
  }: {
    spec?: ConditionSpec;
    /** The scales a score range names, from the catalogue. */
    scales?: ScoreScale[];
    value: unknown;
    /**
     * What the library carries on this condition's axis. Empty when the catalogue names no axis,
     * which is what selects the free-text input below.
     */
    suggestions?: Facet[];
    suggestionsLoading?: boolean;
    suggestionsError?: string | null;
    /** The element that qualifies a list, as the fixed "any of" beside it. */
    describedBy?: string;
    /**
     * `unreadable` when the field holds text `value` could not take in, an id or a number that is
     * not one: the editor holds Save while any field says so.
     */
    onChange: (value: unknown, unreadable: boolean) => void;
  } = $props();

  const range = $derived(
    (value ?? {}) as {
      min?: number | null;
      max?: number | null;
      scale?: string;
      min_votes?: number | null;
    },
  );
  const chosenScale = $derived(scales.find((scale) => scale.id === range.scale) ?? scales[0]);

  /** A score bound neither empty nor a number on the scale keeps the value it had. */
  let scoreUnreadable = $state({ min: false, max: false, votes: false });
  function typeScore(bound: 'min' | 'max', input: HTMLInputElement) {
    const typed = input.value.trim();
    const read = parseScoreBound(typed);
    scoreUnreadable[bound] = input.validity.badInput || (typed !== '' && read === null);
    const next = scoreUnreadable[bound] ? range[bound] : read;
    onChange({ ...range, [bound]: next }, Object.values(scoreUnreadable).some(Boolean));
  }
  function typeVotes(input: HTMLInputElement) {
    const typed = input.value.trim();
    const read = parseWholeBound(typed);
    // Below one is the server's to refuse, with the reason under the rule.
    scoreUnreadable.votes = input.validity.badInput || (typed !== '' && read === null);
    const next = scoreUnreadable.votes ? range.min_votes : read;
    onChange({ ...range, min_votes: next }, Object.values(scoreUnreadable).some(Boolean));
  }
  function chooseScale(id: string) {
    const scale = scales.find((each) => each.id === id);
    // A vote count means nothing on a scale that counts none, and the server refuses it.
    if (!scale?.votes) scoreUnreadable.votes = false;
    onChange(
      { ...range, scale: id, min_votes: scale?.votes ? range.min_votes : null },
      Object.values(scoreUnreadable).some(Boolean),
    );
  }
  const list = $derived(Array.isArray(value) ? (value as string[]) : []);

  /**
   * The list as typed, kept while it still parses to the value: rewritten at each keystroke, it
   * would come back spaced with the cursor at the end, where the next digits run into the last
   * id. A value that stops matching the text, as on a reset, replaces it.
   */
  let typed = $state<string | null>(null);
  const parseList = (text: string): unknown[] =>
    spec?.value_type === 'number_list' ? parseNumberList(text) : parseStringList(text);
  const sameList = (a: unknown[], b: unknown[]) =>
    a.length === b.length && a.every((item, index) => item === b[index]);
  const listText = $derived(
    typed !== null && sameList(parseList(typed), list) ? typed : list.join(', '),
  );

  // Named under the field, or `603, 6O4` would save one id and never route the second title.
  const errorId = $props.id();
  const rejected = $derived(
    spec?.value_type === 'number_list' && typed !== null ? rejectedNumbers(typed) : [],
  );

  function typeList(text: string) {
    typed = text;
    const unreadable = spec?.value_type === 'number_list' && rejectedNumbers(text).length > 0;
    onChange(parseList(text), unreadable);
  }

  /**
   * A number field emptied or unreadable keeps the value it had: sent as `0`, it would store a
   * rule asking for nothing the reader wrote.
   */
  let numberUnreadable = $state(false);
  function typeNumber(input: HTMLInputElement) {
    numberUnreadable = input.value === '' || input.validity.badInput;
    onChange(numberUnreadable ? value : Number(input.value), numberUnreadable);
  }

  /** A year range's bounds, or a runtime's in minutes, as the rule engine accepts them. */
  const bounds = $derived(
    spec?.value_type === 'number_range'
      ? { min: 0, max: MAX_RUNTIME }
      : { min: MIN_YEAR, max: maxYear() },
  );

  /** A bound that is neither empty, which opens it, nor a whole number. */
  const unreadableBound = (input: HTMLInputElement) =>
    input.validity.badInput || (input.value.trim() !== '' && parseWholeBound(input.value) === null);
  let boundsUnreadable = $state({ min: false, max: false });
  function typeBound(bound: 'min' | 'max', input: HTMLInputElement) {
    boundsUnreadable[bound] = unreadableBound(input);
    const next = boundsUnreadable[bound] ? range[bound] : parseWholeBound(input.value);
    onChange({ ...range, [bound]: next }, boundsUnreadable.min || boundsUnreadable.max);
  }
  const yearUnreadable = $derived(boundsUnreadable.min || boundsUnreadable.max);
</script>

{#if spec}
  {#if spec.value_type === 'boolean'}
    <select
      aria-label={spec.label}
      class="form-select"
      value={String(value ?? true)}
      onchange={(event) => onChange(event.currentTarget.value === 'true', false)}
    >
      <option value="true">{t('Yes')}</option>
      <option value="false">{t('No')}</option>
    </select>
  {:else if spec.value_type === 'number'}
    <input
      aria-label={spec.label}
      aria-describedby={numberUnreadable ? errorId : undefined}
      aria-invalid={numberUnreadable ? 'true' : undefined}
      type="number"
      class="form-input"
      value={Number(value ?? 0)}
      oninput={(event) => typeNumber(event.currentTarget)}
    />
    {#if numberUnreadable}
      <p id={errorId} class="field-error">{t('EnterNumber')}</p>
    {/if}
  {:else if spec.value_type === 'number_list'}
    <input
      aria-label={spec.label}
      aria-describedby={[describedBy, rejected.length > 0 ? errorId : undefined]
        .filter(Boolean)
        .join(' ') || undefined}
      aria-invalid={rejected.length > 0 ? 'true' : undefined}
      class="form-input"
      placeholder={t('PlaceholderNumberList')}
      value={listText}
      oninput={(event) => typeList(event.currentTarget.value)}
    />
    {#if rejected.length > 0}
      <p id={errorId} class="field-error">
        {t('NotIdentifiers', { values: rejected.join(t('ListSeparator')) })}
      </p>
    {/if}
  {:else if spec.value_type === 'year_range' || spec.value_type === 'number_range'}
    <div class="flex gap-2">
      <input
        aria-describedby={boundsUnreadable.min ? errorId : undefined}
        aria-invalid={boundsUnreadable.min ? 'true' : undefined}
        // Two controls for one condition: the bound is what tells their names apart.
        aria-label="{spec.label} – {t('PlaceholderYearFrom')}"
        type="number"
        class="form-input"
        min={bounds.min}
        max={bounds.max}
        step="1"
        placeholder={t('PlaceholderYearFrom')}
        value={range.min ?? ''}
        oninput={(event) => typeBound('min', event.currentTarget)}
      />
      <input
        aria-describedby={boundsUnreadable.max ? errorId : undefined}
        aria-invalid={boundsUnreadable.max ? 'true' : undefined}
        aria-label="{spec.label} – {t('PlaceholderYearTo')}"
        type="number"
        class="form-input"
        min={bounds.min}
        max={bounds.max}
        step="1"
        placeholder={t('PlaceholderYearTo')}
        value={range.max ?? ''}
        oninput={(event) => typeBound('max', event.currentTarget)}
      />
    </div>
    {#if yearUnreadable}
      <p id={errorId} class="field-error">{t('EnterWholeNumber')}</p>
    {/if}
  {:else if spec.value_type === 'score_range'}
    <div class="flex flex-col gap-2">
      <select
        aria-label="{spec.label} – {t('ScoreScale')}"
        class="form-select"
        value={chosenScale?.id ?? ''}
        onchange={(event) => chooseScale(event.currentTarget.value)}
      >
        {#each scales as scale (scale.id)}
          <option value={scale.id}>{scale.name}</option>
        {/each}
      </select>
      <div class="flex gap-2">
        <input
          aria-describedby={scoreUnreadable.min ? errorId : undefined}
          aria-invalid={scoreUnreadable.min ? 'true' : undefined}
          aria-label="{spec.label} – {t('PlaceholderYearFrom')}"
          type="number"
          class="form-input"
          min="0"
          max={chosenScale?.max}
          step={chosenScale?.max === 10 ? '0.1' : '1'}
          placeholder={t('PlaceholderYearFrom')}
          value={range.min ?? ''}
          oninput={(event) => typeScore('min', event.currentTarget)}
        />
        <input
          aria-describedby={scoreUnreadable.max ? errorId : undefined}
          aria-invalid={scoreUnreadable.max ? 'true' : undefined}
          aria-label="{spec.label} – {t('PlaceholderYearTo')}"
          type="number"
          class="form-input"
          min="0"
          max={chosenScale?.max}
          step={chosenScale?.max === 10 ? '0.1' : '1'}
          placeholder={t('PlaceholderYearTo')}
          value={range.max ?? ''}
          oninput={(event) => typeScore('max', event.currentTarget)}
        />
      </div>
      {#if scoreUnreadable.min || scoreUnreadable.max}
        <p id={errorId} class="field-error">{t('EnterNumber')}</p>
      {/if}
      {#if chosenScale?.votes}
        <!-- Labelled in sight: a filled count hides the placeholder that would name it. -->
        <div class="flex flex-col gap-1">
          <label class="text-muted text-sm" for="{errorId}-votes-field"
            >{t('ScoreMinimumVotes')}</label
          >
          <input
            id="{errorId}-votes-field"
            aria-describedby="{errorId}-votes{scoreUnreadable.votes ? ` ${errorId}-whole` : ''}"
            aria-invalid={scoreUnreadable.votes ? 'true' : undefined}
            aria-label="{spec.label} – {t('ScoreMinimumVotes')}"
            type="number"
            class="form-input"
            min="1"
            step="1"
            value={range.min_votes ?? ''}
            oninput={(event) => typeVotes(event.currentTarget)}
          />
          <p id="{errorId}-votes" class="form-hint">{t('ScoreMinimumVotesHint')}</p>
          {#if scoreUnreadable.votes}
            <p id="{errorId}-whole" class="field-error">{t('EnterWholeNumber')}</p>
          {/if}
        </div>
      {/if}
    </div>
  {:else if spec.value_type === 'string'}
    <input
      aria-label={spec.label}
      class="form-input mono"
      placeholder={t('PlaceholderPath')}
      value={String(value ?? '')}
      oninput={(event) => onChange(event.currentTarget.value, false)}
    />
  {:else if spec.suggestions}
    <ValuePicker
      label={spec.label}
      values={list}
      options={suggestions}
      loading={suggestionsLoading}
      error={suggestionsError}
      {describedBy}
      keyOf={keyOfKind(spec.type)}
      onChange={(values) => onChange(values, false)}
    />
  {:else}
    <input
      aria-label={spec.label}
      aria-describedby={describedBy}
      class="form-input"
      placeholder={t('PlaceholderStringList')}
      value={listText}
      oninput={(event) => typeList(event.currentTarget.value)}
    />
  {/if}
{/if}
