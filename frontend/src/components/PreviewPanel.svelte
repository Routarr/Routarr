<script lang="ts">
  import type { RulePreview } from '../api/types';
  import { t } from '../lib/i18n.svelte';
  import Confidence from './Confidence.svelte';
  import TableRegion from './TableRegion.svelte';

  let { preview }: { preview: RulePreview } = $props();
</script>

<div class="card card-inset mt-4">
  <div class="card-header">
    <h3 class="card-title">{t('PreviewTitle')}</h3>
  </div>

  {#each preview.issues as issue, index (index)}
    <div class="banner {issue.severity === 'error' ? 'banner-danger' : 'banner-warning'}">
      <span>{issue.message}</span>
    </div>
  {/each}

  <p class="text-base mb-3">
    {t('PreviewSummary', {
      changed: preview.changed_total,
      beforeMoves: preview.before.moves_required,
      afterMoves: preview.after.moves_required,
      beforeUnmatched: preview.before.no_category_match,
      afterUnmatched: preview.after.no_category_match,
    })}
  </p>

  {#if preview.changed.length > 0}
    <TableRegion label={t('PreviewTitle')}>
      <table>
        <caption class="visually-hidden">{t('PreviewTitle')}</caption>
        <thead>
          <tr>
            <th>{t('Media')}</th>
            <th>{t('PreviewFrom')}</th>
            <th>{t('PreviewTo')}</th>
            <th class="w-90">{t('Confidence')}</th>
          </tr>
        </thead>
        <tbody>
          {#each preview.changed as change (change.media_id)}
            <tr>
              <td>{change.media_title}</td>
              <td><span class="badge badge-value muted">{change.from_category}</span></td>
              <td><span class="badge badge-value">{change.to_category}</span></td>
              <td><Confidence value={change.confidence} /></td>
            </tr>
          {/each}
        </tbody>
      </table>
      {#if preview.changed_total > preview.changed.length}
        <p class="text-muted mt-2 text-sm">
          {t('PreviewTruncated', {
            shown: preview.changed.length,
            total: preview.changed_total,
          })}
        </p>
      {/if}
    </TableRegion>
  {/if}
</div>
