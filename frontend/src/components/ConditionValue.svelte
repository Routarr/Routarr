<script lang="ts">
  import type { ConditionSpec, Facet } from '../api/types';
  import {
    MIN_YEAR,
    maxYear,
    parseNumberList,
    rejectedNumbers,
    parseStringList,
    parseYearBound,
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
    onChange,
  }: {
    spec?: ConditionSpec;
    value: unknown;
    /**
     * What the library carries on this condition's axis. Empty for a condition
     * the catalogue names no axis for, which is what selects the free-text
     * input below.
     */
    suggestions?: Facet[];
    suggestionsLoading?: boolean;
    suggestionsError?: string | null;
    /** The element that qualifies a list, as the fixed "any of" beside it. */
    describedBy?: string;
    /**
     * `unreadable` when the field holds text that `value` could not take in,
     * an id that is not one or a number that is not one: the editor holds
     * Save while any field says so.
     */
    onChange: (value: unknown, unreadable: boolean) => void;
  } = $props();

  const range = $derived((value ?? {}) as { min?: number | null; max?: number | null });
  const list = $derived(Array.isArray(value) ? (value as string[]) : []);

  /**
   * A free list as the reader typed it, kept while it still reads as the value.
   * Rewritten from the parsed list at each keystroke, a list typed without
   * spaces would come back spaced, and the browser would send the cursor to the
   * end, where the next digits run into the last id. A value that stops
   * matching the text, as when the condition is reset, replaces it.
   */
  let typed = $state<string | null>(null);
  const parseList = (text: string): unknown[] =>
    spec?.value_type === 'number_list' ? parseNumberList(text) : parseStringList(text);
  const sameList = (a: unknown[], b: unknown[]) =>
    a.length === b.length && a.every((item, index) => item === b[index]);
  const listText = $derived(
    typed !== null && sameList(parseList(typed), list) ? typed : list.join(', '),
  );

  // Named under the field rather than dropped: `603, 6O4` would otherwise save
  // one id, and the second title would never be routed.
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
   * A number field emptied, or holding what the browser cannot read as one,
   * keeps the value it had: sent as `0`, it would store a rule asking for
   * nothing the reader wrote.
   */
  let numberUnreadable = $state(false);
  function typeNumber(input: HTMLInputElement) {
    numberUnreadable = input.value === '' || input.validity.badInput;
    onChange(numberUnreadable ? value : Number(input.value), numberUnreadable);
  }

  /** A bound that is neither empty, which opens it, nor a whole year. */
  const unreadableBound = (input: HTMLInputElement) =>
    input.validity.badInput || (input.value.trim() !== '' && parseYearBound(input.value) === null);
  let boundsUnreadable = $state({ min: false, max: false });
  function typeBound(bound: 'min' | 'max', input: HTMLInputElement) {
    boundsUnreadable[bound] = unreadableBound(input);
    const next = boundsUnreadable[bound] ? range[bound] : parseYearBound(input.value);
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
  {:else if spec.value_type === 'year_range'}
    <div class="flex gap-2">
      <input
        aria-describedby={boundsUnreadable.min ? errorId : undefined}
        aria-invalid={boundsUnreadable.min ? 'true' : undefined}
        // Two controls for one condition, so the caption alone would name them
        // identically: the bound is what tells them apart.
        aria-label="{spec.label} – {t('PlaceholderYearFrom')}"
        type="number"
        class="form-input"
        min={MIN_YEAR}
        max={maxYear()}
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
        min={MIN_YEAR}
        max={maxYear()}
        step="1"
        placeholder={t('PlaceholderYearTo')}
        value={range.max ?? ''}
        oninput={(event) => typeBound('max', event.currentTarget)}
      />
    </div>
    {#if yearUnreadable}
      <p id={errorId} class="field-error">{t('EnterWholeNumber')}</p>
    {/if}
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
