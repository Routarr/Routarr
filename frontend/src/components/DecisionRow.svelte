<script lang="ts">
  import { AlertTriangle, ArrowRight, Ban, Check, Lock } from '../lib/icons';
  import type { Decision } from '../api/types';
  import { mediaTypeKey } from '../api/format';
  import { t } from '../lib/i18n.svelte';
  import NoValue from './NoValue.svelte';
  import Confidence from './Confidence.svelte';

  let {
    decision,
    selected,
    onToggle,
    onKeep,
  }: {
    decision: Decision;
    selected: boolean;
    /** Selects the move for an apply, which a reader is offered no box for. */
    onToggle?: () => void;
    /** Keeps the title where it is, offered on a pending proposal whose folder has a category. */
    onKeep?: () => void;
  } = $props();

  const keepable = $derived(
    onKeep !== undefined &&
      decision.status === 'pending' &&
      decision.action !== 'none' &&
      decision.current_category !== null,
  );

  // A clamped justification needs its button: the title shows the rest to a mouse alone.
  let reasons = $state<HTMLElement>();
  let clamped = $state(false);
  let expanded = $state(false);
  // Deferred a frame: toggling the button inside the callback is a ResizeObserver loop error.
  $effect(() => {
    const box = reasons;
    if (!box) return;
    let frame = 0;
    const observer = new ResizeObserver(() => {
      cancelAnimationFrame(frame);
      frame = requestAnimationFrame(() => {
        clamped = [...box.querySelectorAll('.reason-line')].some(
          (line) => line.scrollHeight > line.clientHeight,
        );
      });
    });
    observer.observe(box);
    return () => {
      cancelAnimationFrame(frame);
      observer.disconnect();
    };
  });
</script>

<tr>
  <td>
    {#if decision.action === 'move' && onToggle}
      <input
        type="checkbox"
        checked={selected}
        onchange={onToggle}
        aria-label={t('SelectMoveFor', { title: decision.media_title })}
      />
    {/if}
  </td>
  <td>
    <strong class="cell-title">{decision.media_title}</strong>
    <div class="text-muted text-sm">
      {decision.instance_name} · {t(mediaTypeKey(decision.media_type))}
      {#if decision.is_override}
        <span class="badge badge-warning ms-1">
          {t('ManualOverride')}
        </span>
      {/if}
    </div>
    {#if keepable}
      <button
        class="btn btn-ghost btn-sm mt-1"
        onclick={onKeep}
        aria-label="{t('KeepHere')} – {decision.media_title}"
      >
        <Lock size={12} aria-hidden="true" />
        {t('KeepHere')}
      </button>
    {/if}
  </td>
  <td class="mono text-sm">
    <span class="cell-path">
      {#if decision.current_root_folder}
        <bdi>{decision.current_root_folder}</bdi>
      {:else}
        <NoValue />
      {/if}
    </span>
  </td>
  <td>
    <!-- A screen reader reads an unnamed icon as nothing,
         so the word stays beside the shape and the colour. -->
    {#if decision.action === 'move'}
      <ArrowRight size={16} class="text-warning dir-aware" aria-hidden="true" />
      <span class="visually-hidden">{t('ActionMove')}</span>
    {:else if decision.action === 'skip'}
      <Ban size={16} class="text-danger" aria-hidden="true" />
      <span class="visually-hidden">{t('ActionSkip')}</span>
    {:else}
      <Check size={16} class="text-success" aria-hidden="true" />
      <span class="visually-hidden">{t('ActionNone')}</span>
    {/if}
  </td>
  <td class="mono text-sm">
    {#if decision.target_root_folder}
      <span class="cell-path">
        <bdi>{decision.target_root_folder}</bdi>
      </span>
    {:else}
      <span class="text-danger">
        <AlertTriangle size={12} />
        {t('NoFolderForCategory', { category: decision.target_category })}
      </span>
    {/if}
  </td>
  <td>
    <span class="badge badge-value muted">
      {decision.matched_rule_name ?? t('DefaultCategoryFallback')}
    </span>
  </td>
  <td><Confidence value={decision.confidence} /></td>
  <td>
    <div
      id="reasons-{decision.id}"
      class="cell-reasons flex flex-col gap-1"
      class:is-expanded={expanded}
      bind:this={reasons}
    >
      {#each decision.reason_lines as reason, index (index)}
        <span
          class="reason-line {reason.matched ? 'explain-match' : 'text-muted'}"
          title={reason.text}
        >
          {reason.text}
        </span>
      {/each}
      {#each decision.alternatives as alternative, index (index)}
        <span class="text-muted text-xs">
          {#if alternative.excluded_by}
            ⛔ {t('ExcludedAlternative', {
              rule: alternative.rule_name,
              reason: alternative.excluded_by,
            })}
          {:else}
            <!-- Mirrored with the text: it points from the rule above to this one. -->
            <span class="dir-aware" aria-hidden="true">↳</span>
            {t('AlsoMatched', { rule: alternative.rule_name, category: alternative.category })}
          {/if}
        </span>
      {/each}
    </div>
    {#if clamped || expanded}
      <button
        type="button"
        class="btn btn-ghost btn-sm mt-1"
        aria-label="{t(expanded ? 'ReasonsShowLess' : 'ReasonsShowAll')} – {decision.media_title}"
        aria-expanded={expanded}
        aria-controls="reasons-{decision.id}"
        onclick={() => (expanded = !expanded)}
      >
        {t(expanded ? 'ReasonsShowLess' : 'ReasonsShowAll')}
      </button>
    {/if}
  </td>
</tr>
