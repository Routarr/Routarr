<script lang="ts">
  import { AlertTriangle, ArrowRight, Ban, Check } from '../lib/icons';
  import type { Decision } from '../api/types';
  import { mediaTypeKey } from '../api/format';
  import { t } from '../lib/i18n.svelte';
  import Confidence from './Confidence.svelte';

  let {
    decision,
    selected,
    onToggle,
  }: { decision: Decision; selected: boolean; onToggle: () => void } = $props();

  // A justification is clamped to two lines, so a row keeps the height of its
  // neighbours. A clamped one is offered in full behind a button: its title
  // shows the rest to a mouse alone.
  let reasons = $state<HTMLElement>();
  let clamped = $state(false);
  let expanded = $state(false);
  $effect(() => {
    const box = reasons;
    if (!box) return;
    const observer = new ResizeObserver(() => {
      clamped = [...box.querySelectorAll('.reason-line')].some(
        (line) => line.scrollHeight > line.clientHeight,
      );
    });
    observer.observe(box);
    return () => observer.disconnect();
  });
</script>

<tr>
  <td>
    {#if decision.action === 'move'}
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
  </td>
  <td class="mono text-sm">
    <span class="cell-path">
      <bdi>{decision.current_root_folder ?? t('None')}</bdi>
    </span>
  </td>
  <td>
    <!-- The shape and the colour for the eye, the word for a screen reader,
         which reads an unnamed icon as nothing. -->
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
      class="flex flex-col gap-1"
      class:is-expanded={expanded}
      bind:this={reasons}
    >
      {#each decision.reasons as reason, index (index)}
        <span
          class="reason-line {reason.startsWith('✓') ? 'explain-match' : 'text-muted'}"
          title={reason}
        >
          {reason}
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
        aria-expanded={expanded}
        aria-controls="reasons-{decision.id}"
        onclick={() => (expanded = !expanded)}
      >
        {t(expanded ? 'ReasonsShowLess' : 'ReasonsShowAll')}
      </button>
    {/if}
  </td>
</tr>
