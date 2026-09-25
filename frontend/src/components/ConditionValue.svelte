<script lang="ts">
  import type { ConditionSpec, Facet } from '../api/types';
  import {
    MIN_YEAR,
    maxYear,
    parseNumberList,
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
    onChange,
  }: {
    spec?: ConditionSpec;
    value: unknown;
    /// What the library carries on this condition's axis. Empty for a condition
    /// the catalogue names no axis for, which is what selects the free-text
    /// input below.
    suggestions?: Facet[];
    suggestionsLoading?: boolean;
    suggestionsError?: string | null;
    onChange: (value: unknown) => void;
  } = $props();

  const range = $derived((value ?? {}) as { min?: number | null; max?: number | null });
  const list = $derived(Array.isArray(value) ? (value as string[]) : []);
</script>

{#if spec}
  {#if spec.value_type === 'boolean'}
    <select
      aria-label={spec.label}
      class="form-select"
      value={String(value ?? true)}
      onchange={(event) => onChange(event.currentTarget.value === 'true')}
    >
      <option value="true">{t('Yes')}</option>
      <option value="false">{t('No')}</option>
    </select>
  {:else if spec.value_type === 'number'}
    <input
      aria-label={spec.label}
      type="number"
      class="form-input"
      value={Number(value ?? 0)}
      oninput={(event) => onChange(Number(event.currentTarget.value))}
    />
  {:else if spec.value_type === 'number_list'}
    <input
      aria-label={spec.label}
      class="form-input"
      placeholder={t('PlaceholderNumberList')}
      value={Array.isArray(value) ? (value as number[]).join(', ') : ''}
      oninput={(event) => onChange(parseNumberList(event.currentTarget.value))}
    />
  {:else if spec.value_type === 'year_range'}
    <div class="flex gap-2">
      <input
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
        oninput={(event) => onChange({ ...range, min: parseYearBound(event.currentTarget.value) })}
      />
      <input
        aria-label="{spec.label} – {t('PlaceholderYearTo')}"
        type="number"
        class="form-input"
        min={MIN_YEAR}
        max={maxYear()}
        step="1"
        placeholder={t('PlaceholderYearTo')}
        value={range.max ?? ''}
        oninput={(event) => onChange({ ...range, max: parseYearBound(event.currentTarget.value) })}
      />
    </div>
  {:else if spec.value_type === 'string'}
    <input
      aria-label={spec.label}
      class="form-input"
      placeholder={t('PlaceholderPath')}
      value={String(value ?? '')}
      oninput={(event) => onChange(event.currentTarget.value)}
    />
  {:else if spec.suggestions}
    <ValuePicker
      label={spec.label}
      values={list}
      options={suggestions}
      loading={suggestionsLoading}
      error={suggestionsError}
      onChange={(values) => onChange(values)}
    />
  {:else}
    <input
      aria-label={spec.label}
      class="form-input"
      placeholder={t('PlaceholderStringList')}
      value={list.join(', ')}
      oninput={(event) => onChange(parseStringList(event.currentTarget.value))}
    />
  {/if}
{/if}
