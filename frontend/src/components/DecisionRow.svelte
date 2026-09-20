<script lang="ts">
  import { AlertTriangle, ArrowRight, Ban, Check } from '../lib/icons';
  import type { Decision } from '../api/types';
  import { t } from '../lib/i18n.svelte';
  import Confidence from './Confidence.svelte';

  let {
    decision,
    selected,
    onToggle,
  }: { decision: Decision; selected: boolean; onToggle: () => void } = $props();
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
    <strong>{decision.media_title}</strong>
    <div class="text-muted text-sm">
      {decision.instance_name} · {t(decision.media_type === 'movie' ? 'Movies' : 'Series')}
      {#if decision.is_override}
        <span class="badge badge-warning ms-1">
          {t('ManualOverride')}
        </span>
      {/if}
    </div>
  </td>
  <td class="mono text-sm">{decision.current_root_folder ?? t('None')}</td>
  <td>
    {#if decision.action === 'move'}
      <ArrowRight size={16} class="text-warning" />
    {:else if decision.action === 'skip'}
      <Ban size={16} class="text-danger" />
    {:else}
      <Check size={16} class="text-success" />
    {/if}
  </td>
  <td class="mono text-sm">
    {#if decision.target_root_folder}
      {decision.target_root_folder}
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
    <div class="flex flex-col gap-1">
      {#each decision.reasons as reason, index (index)}
        <!-- Clamped, with the whole sentence on hover: a justification is
             several lines of prose in a narrow column, and one row in three was
             twice the height of its neighbours. -->
        <span
          class="reason-line {reason.startsWith('✓') ? 'explain-match' : 'text-muted'}"
          title={reason}
        >
          {reason}
        </span>
      {/each}
      {#each decision.alternatives as alternative, index (index)}
        <span class="text-muted text-xs">
          {alternative.excluded_by
            ? `⛔ ${t('ExcludedAlternative', { rule: alternative.rule_name, reason: alternative.excluded_by })}`
            : `↳ ${t('AlsoMatched', { rule: alternative.rule_name, category: alternative.category })}`}
        </span>
      {/each}
    </div>
  </td>
</tr>
