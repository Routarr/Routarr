<script lang="ts">
  import { t } from '../lib/i18n.svelte';

  /**
   * Placeholder rows while a table loads.
   *
   * A spinner in the middle of an empty card says "wait"; it does not say what
   * is coming or how tall it will be, so the page jumps when the data lands.
   * These hold the shape. `aria-hidden` because a screen reader is told the
   * table is busy, not shown grey rectangles.
   */
  let { columns, rows = 6 }: { columns: number; rows?: number } = $props();
</script>

<tr>
  <td colspan={columns}>
    <span class="visually-hidden" role="status">{t('Loading')}</span>
  </td>
</tr>
{#each { length: rows } as _, row (row)}
  <tr aria-hidden="true">
    {#each { length: columns } as _, column (column)}
      <td>
        <div class="skeleton skeleton-line" style="width: {column === 0 ? '60%' : '40%'}"></div>
      </td>
    {/each}
  </tr>
{/each}
